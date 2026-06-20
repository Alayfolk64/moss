use alloc::{borrow::Cow, sync::Arc, vec, vec::Vec};
use core::{
    ffi::{c_char, c_int},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, FileBackend, FileFlags, OpenOptions, update_cached_metadata};
use axfs_ng_vfs::{MetadataUpdate, NodeType};
use axhal::time::realtime_time;
use axio::{IoBuf, Read, Seek, SeekFrom};
use axpoll::{IoEvents, Pollable};
use axtask::current;
use linux_raw_sys::general::{
    __kernel_off_t, AT_FDCWD, FALLOC_FL_ALLOCATE_RANGE, FALLOC_FL_COLLAPSE_RANGE,
    FALLOC_FL_INSERT_RANGE, FALLOC_FL_KEEP_SIZE, FALLOC_FL_PUNCH_HOLE, FALLOC_FL_ZERO_RANGE,
    FS_IMMUTABLE_FL, IN_ACCESS, IN_MODIFY, MS_RDONLY, O_ACCMODE, O_WRONLY, POSIX_FADV_WILLNEED,
    RLIM_INFINITY, RLIMIT_FSIZE, SPLICE_F_GIFT, SPLICE_F_MORE, SPLICE_F_MOVE, SPLICE_F_NONBLOCK,
};
use starry_signal::Signo;
use starry_vm::{VmMutPtr, VmPtr};
use syscalls::Sysno;

use super::{
    fanotify::{
        check_fanotify_access_permission, notify_fanotify_access, notify_fanotify_modify,
        notify_fanotify_modify_path,
    },
    inotify::{notify_inotify_file, notify_inotify_path},
    memfd::{check_memfd_resize, check_memfd_write, check_memfd_write_at, memfd_state},
};
#[cfg(feature = "net")]
use crate::file::Socket;
use crate::{
    file::{Directory, File, FileLike, IoDst, Pipe, get_file_like, inode_flags},
    mm::{IoVec, IoVectorBuf, UserConstPtr, VmBytes, VmBytesMut},
    task::AsThread,
};

struct DummyFd;
impl FileLike for DummyFd {
    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[dummy]".into()
    }
}
impl Pollable for DummyFd {
    fn poll(&self) -> IoEvents {
        IoEvents::empty()
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

pub fn sys_dummy_fd(sysno: Sysno) -> AxResult<isize> {
    if current().name().starts_with("qemu-") {
        // We need to be honest to qemu, since it can automatically fallback to
        // other strategies.
        return Err(AxError::Unsupported);
    }
    warn!("Dummy fd created: {sysno}");
    DummyFd.add_to_fd_table(false).map(|fd| fd as isize)
}

#[cold]
fn should_retry_interrupted_read() -> bool {
    let curr = current();
    let thread = curr.as_thread();
    let signal = &thread.signal;
    let pending = signal.pending() & !signal.blocked();
    if pending.is_empty() {
        return true;
    }

    (1u8..=64)
        .filter_map(Signo::from_repr)
        .filter(|&signo| pending.has(signo))
        .all(|signo| thread.proc_data.signal.can_restart(signo))
}

fn read_file_like(f: &Arc<dyn FileLike>, dst: &mut IoDst<'_>) -> AxResult<usize> {
    loop {
        match f.read(dst) {
            Err(AxError::Interrupted) if should_retry_interrupted_read() => {}
            result => return result,
        }
    }
}

/// Read data from the file indicated by `fd`.
///
/// Return the read size if success.
pub fn sys_read(fd: i32, buf: *mut u8, len: usize) -> AxResult<isize> {
    debug!("sys_read <= fd: {fd}, buf: {buf:p}, len: {len}");
    let f = get_file_like(fd)?;
    if len > 0 {
        check_fanotify_access_permission(&f)?;
    }
    let read = read_file_like(&f, &mut VmBytesMut::new(buf, len))?;
    if read > 0 {
        notify_inotify_file(f.as_ref(), IN_ACCESS);
        notify_fanotify_access(&f);
    }
    Ok(read as _)
}

pub fn sys_readv(fd: i32, iov: *const IoVec, iovcnt: usize) -> AxResult<isize> {
    debug!("sys_readv <= fd: {fd}, iovcnt: {iovcnt}");
    let f = get_file_like(fd)?;
    let iov = IoVectorBuf::new(iov, iovcnt)?;
    if iovcnt > 0 {
        check_fanotify_access_permission(&f)?;
    }
    let read = read_file_like(&f, &mut iov.into_io())?;
    if read > 0 {
        notify_inotify_file(f.as_ref(), IN_ACCESS);
        notify_fanotify_access(&f);
    }
    Ok(read as _)
}

/// Write data to the file indicated by `fd`.
///
/// Return the written size if success.
pub fn sys_write(fd: i32, buf: *const u8, len: usize) -> AxResult<isize> {
    debug!("sys_write <= fd: {fd}, buf: {buf:p}, len: {len}");
    if len == 0 {
        return Ok(0);
    }
    let f = get_file_like(fd)?;
    let mut len = len;
    if let Some(file) = f.downcast_ref::<File>() {
        check_memfd_write(file)?;
        len = limit_current_write_len(file, len)?;
    }
    let written = f.write(&mut VmBytes::new(buf, len))?;
    if written > 0 {
        notify_inotify_file(f.as_ref(), IN_MODIFY);
        notify_fanotify_modify(&f);
    }
    Ok(written as _)
}

pub fn sys_writev(fd: i32, iov: *const IoVec, iovcnt: usize) -> AxResult<isize> {
    debug!("sys_writev <= fd: {fd}, iovcnt: {iovcnt}");
    let f = get_file_like(fd)?;
    let data = read_iov_to_vec(iov, iovcnt)?;
    let mut len = data.len();
    if let Some(file) = f.downcast_ref::<File>() {
        check_memfd_write(file)?;
        len = limit_current_write_len(file, len)?;
    }
    let mut data = &data[..len];
    let written = f.write(&mut data)?;
    if written > 0 {
        notify_inotify_file(f.as_ref(), IN_MODIFY);
        notify_fanotify_modify(&f);
    }
    Ok(written as _)
}

fn file_size_limit() -> u64 {
    current().as_thread().proc_data.rlim.read()[RLIMIT_FSIZE].current
}

fn check_new_file_size(size: u64) -> AxResult {
    let limit = file_size_limit();
    if limit != RLIM_INFINITY as u64 && size > limit {
        Err(AxError::from(LinuxError::EFBIG))
    } else {
        Ok(())
    }
}

fn limit_write_len_at(offset: u64, len: usize) -> AxResult<usize> {
    let limit = file_size_limit();
    if len == 0 || limit == RLIM_INFINITY as u64 {
        return Ok(len);
    }
    if offset >= limit {
        return Err(AxError::from(LinuxError::EFBIG));
    }
    Ok(len.min((limit - offset) as usize))
}

fn current_write_offset(file: &File) -> AxResult<u64> {
    if file.inner().flags().contains(FileFlags::APPEND) {
        file.inner().len()
    } else {
        file.inner().seek(SeekFrom::Current(0))
    }
}

fn limit_current_write_len(file: &File, len: usize) -> AxResult<usize> {
    limit_write_len_at(current_write_offset(file)?, len)
}

fn limit_positioned_write_len(file: &File, offset: u64, len: usize) -> AxResult<usize> {
    let offset = if file.inner().flags().contains(FileFlags::APPEND) {
        file.inner().len()?
    } else {
        offset
    };
    limit_write_len_at(offset, len)
}

fn read_iov_to_vec(iov: *const IoVec, iovcnt: usize) -> AxResult<Vec<u8>> {
    let mut iov = IoVectorBuf::new(iov, iovcnt)?.into_io();
    let mut data = vec![0; iov.remaining()];
    iov.read(&mut data)?;
    Ok(data)
}

pub fn sys_lseek(fd: c_int, offset: __kernel_off_t, whence: c_int) -> AxResult<isize> {
    debug!("sys_lseek <= {fd} {offset} {whence}");
    let pos = match whence {
        0 => SeekFrom::Start(offset as _),
        1 => SeekFrom::Current(offset as _),
        2 => SeekFrom::End(offset as _),
        _ => return Err(AxError::InvalidInput),
    };
    let off = File::from_fd(fd)?.inner().seek(pos)?;
    Ok(off as _)
}

pub fn sys_truncate(path: UserConstPtr<c_char>, length: __kernel_off_t) -> AxResult<isize> {
    let path = path.get_as_str()?;
    debug!("sys_truncate <= {path:?} {length}");
    if path.is_empty() {
        return Err(AxError::NotFound);
    }
    if length < 0 {
        return Err(AxError::InvalidInput);
    }
    let loc = FS_CONTEXT.lock().resolve(path)?;
    let metadata = loc.metadata()?;
    if metadata.node_type == NodeType::Directory {
        return Err(AxError::IsADirectory);
    }
    if loc.mount_flags() & MS_RDONLY != 0 {
        return Err(AxError::ReadOnlyFilesystem);
    }
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let euid = proc_data.euid();
    if euid != 0
        && !can_write_file(
            metadata.mode.bits() as u32,
            metadata.uid,
            metadata.gid,
            euid,
            proc_data.egid(),
        )
    {
        return Err(AxError::PermissionDenied);
    }
    check_new_file_size(length as u64)?;
    let file = OpenOptions::new()
        .write(true)
        .open(&FS_CONTEXT.lock(), path)?
        .into_file()?;
    file.access(FileFlags::WRITE)?.set_len(length as _)?;
    notify_inotify_path(AT_FDCWD, path, IN_MODIFY);
    notify_fanotify_modify_path(AT_FDCWD, path);
    Ok(0)
}

fn can_write_file(mode: u32, uid: u32, gid: u32, euid: u32, egid: u32) -> bool {
    let shift = if euid == uid {
        6
    } else if egid == gid {
        3
    } else {
        0
    };
    mode & (0o2 << shift) != 0
}

pub fn sys_ftruncate(fd: c_int, length: __kernel_off_t) -> AxResult<isize> {
    debug!("sys_ftruncate <= {fd} {length}");
    if length < 0 {
        return Err(AxError::InvalidInput);
    }
    let f = File::from_fd(fd)?;
    check_new_file_size(length as u64)?;
    check_memfd_resize(&f, length as u64)?;
    f.inner().access(FileFlags::WRITE)?.set_len(length as _)?;
    notify_inotify_file(f.as_ref(), IN_MODIFY);
    let f: Arc<dyn FileLike> = f;
    notify_fanotify_modify(&f);
    Ok(0)
}

pub fn sys_fallocate(
    fd: c_int,
    mode: u32,
    offset: __kernel_off_t,
    len: __kernel_off_t,
) -> AxResult<isize> {
    debug!("sys_fallocate <= fd: {fd}, mode: {mode}, offset: {offset}, len: {len}");
    if offset < 0 || len <= 0 {
        return Err(AxError::InvalidInput);
    }
    let end = (offset as u64)
        .checked_add(len as u64)
        .ok_or(AxError::from(LinuxError::EFBIG))?;

    let f = File::from_fd(fd)?;
    let inner = f.inner();
    let file = inner.access(FileFlags::WRITE)?;
    match mode {
        FALLOC_FL_ALLOCATE_RANGE => {
            let new_len = file.len()?.max(end);
            check_memfd_resize(&f, new_len)?;
            file.set_len(new_len)?;
            Ok(0)
        }
        FALLOC_FL_KEEP_SIZE => Ok(0),
        mode if mode == FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE => {
            check_memfd_write(&f)?;
            if memfd_state(&f).is_none() {
                return Err(AxError::OperationNotSupported);
            }
            zero_existing_file_range(file, offset as u64, end)?;
            Ok(0)
        }
        FALLOC_FL_ZERO_RANGE | FALLOC_FL_INSERT_RANGE | FALLOC_FL_COLLAPSE_RANGE => {
            Err(AxError::OperationNotSupported)
        }
        _ => Err(AxError::InvalidInput),
    }
}

fn zero_existing_file_range(file: &FileBackend, start: u64, end: u64) -> AxResult {
    let mut pos = start;
    let end = end.min(file.len()?);
    let zeros = [0u8; 4096];
    while pos < end {
        let len = (end - pos).min(zeros.len() as u64) as usize;
        let written = file.write_at(&zeros[..len], pos)?;
        if written == 0 {
            return Err(AxError::WriteZero);
        }
        pos += written as u64;
    }
    Ok(())
}

pub fn sys_fsync(fd: c_int) -> AxResult<isize> {
    debug!("sys_fsync <= {fd}");
    let f = File::from_fd(fd)?;
    f.inner().sync(false)?;
    Ok(0)
}

pub fn sys_fdatasync(fd: c_int) -> AxResult<isize> {
    debug!("sys_fdatasync <= {fd}");
    let f = File::from_fd(fd)?;
    f.inner().sync(true)?;
    Ok(0)
}

pub fn sys_sync_file_range(
    fd: c_int,
    offset: __kernel_off_t,
    nbytes: __kernel_off_t,
    flags: u32,
) -> AxResult<isize> {
    const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 1;
    const SYNC_FILE_RANGE_WRITE: u32 = 2;
    const SYNC_FILE_RANGE_WAIT_AFTER: u32 = 4;
    const SYNC_FILE_RANGE_VALID_FLAGS: u32 =
        SYNC_FILE_RANGE_WAIT_BEFORE | SYNC_FILE_RANGE_WRITE | SYNC_FILE_RANGE_WAIT_AFTER;

    debug!("sys_sync_file_range <= fd: {fd}, offset: {offset}, nbytes: {nbytes}, flags: {flags}");
    if offset < 0 || nbytes < 0 || flags & !SYNC_FILE_RANGE_VALID_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }
    if Pipe::from_fd(fd).is_ok() {
        return Err(AxError::from(LinuxError::ESPIPE));
    }

    let f = File::from_fd(fd)?;
    f.inner().sync(false)?;
    Ok(0)
}

fn file_for_readahead(file_like: &Arc<dyn FileLike>) -> AxResult<Option<&File>> {
    if file_like.path().as_ref() == "anon_inode:[pidfd]" {
        return Ok(None);
    }
    if file_like.downcast_ref::<Pipe>().is_some() {
        return Err(AxError::from(LinuxError::ESPIPE));
    }
    let Some(file) = file_like.downcast_ref::<File>() else {
        return Err(AxError::InvalidInput);
    };
    if file.inner().is_path() || file.status_flags() & O_ACCMODE == O_WRONLY {
        return Err(AxError::BadFileDescriptor);
    }
    if file.inner().location().metadata()?.node_type != NodeType::RegularFile {
        return Err(AxError::InvalidInput);
    }
    Ok(Some(file))
}

pub fn sys_fadvise64(
    fd: c_int,
    offset: __kernel_off_t,
    len: __kernel_off_t,
    advice: u32,
) -> AxResult<isize> {
    debug!("sys_fadvise64 <= fd: {fd}, offset: {offset}, len: {len}, advice: {advice}");
    if offset < 0 || len < 0 {
        return Err(AxError::InvalidInput);
    }
    if Pipe::from_fd(fd).is_ok() {
        return Err(AxError::BrokenPipe);
    }
    if advice > 5 {
        return Err(AxError::InvalidInput);
    }
    if advice == POSIX_FADV_WILLNEED && len > 0 {
        let file_like = get_file_like(fd)?;
        if let Some(file) = file_for_readahead(&file_like)? {
            file.inner()
                .backend()?
                .readahead(offset as u64, len as usize)?;
        }
    }
    Ok(0)
}

pub fn sys_readahead(fd: c_int, offset: __kernel_off_t, count: usize) -> AxResult<isize> {
    debug!("sys_readahead <= fd: {fd}, offset: {offset}");
    if offset < 0 {
        return Err(AxError::InvalidInput);
    }

    let file_like = get_file_like(fd)?;
    if let Some(file) = file_for_readahead(&file_like)? {
        file.inner().backend()?.readahead(offset as u64, count)?;
    }
    Ok(0)
}

pub fn sys_pread64(fd: c_int, buf: *mut u8, len: usize, offset: __kernel_off_t) -> AxResult<isize> {
    if offset < 0 {
        return Err(AxError::InvalidInput);
    }
    let f = positioned_file_from_fd(fd)?;
    let read = f.inner().read_at(VmBytesMut::new(buf, len), offset as _)?;
    Ok(read as _)
}

pub fn sys_pwrite64(
    fd: c_int,
    buf: *const u8,
    len: usize,
    offset: __kernel_off_t,
) -> AxResult<isize> {
    if offset < 0 {
        return Err(AxError::InvalidInput);
    }
    let f = positioned_file_from_fd(fd)?;
    let len = limit_positioned_write_len(&f, offset as u64, len)?;
    check_memfd_write_at(&f, offset as u64, len)?;
    if len == 0 {
        return Ok(0);
    }
    let inner = f.inner();
    let write = if let Ok(file) = inner.access(FileFlags::APPEND) {
        file.append(VmBytes::new(buf, len))?.0
    } else {
        inner.write_at(VmBytes::new(buf, len), offset as _)?
    };
    if write > 0 {
        record_positioned_write(&f)?;
    }
    Ok(write as _)
}

fn record_positioned_write(file: &File) -> AxResult {
    let now = realtime_time();
    if !file.inner().record_modification(now) {
        update_cached_metadata(
            file.inner().location(),
            MetadataUpdate {
                mtime: Some(now),
                ctime: Some(now),
                ..Default::default()
            },
        )?;
    }
    Ok(())
}

fn positioned_file_from_fd(fd: c_int) -> AxResult<Arc<File>> {
    File::from_fd(fd).map_err(|err| {
        if err == AxError::BrokenPipe {
            AxError::from(LinuxError::ESPIPE)
        } else {
            err
        }
    })
}

pub fn sys_preadv(
    fd: c_int,
    iov: *const IoVec,
    iovcnt: usize,
    offset: __kernel_off_t,
) -> AxResult<isize> {
    if offset < 0 {
        return Err(AxError::InvalidInput);
    }
    sys_preadv2(fd, iov, iovcnt, offset, 0)
}

pub fn sys_pwritev(
    fd: c_int,
    iov: *const IoVec,
    iovcnt: usize,
    offset: __kernel_off_t,
) -> AxResult<isize> {
    if offset < 0 {
        return Err(AxError::InvalidInput);
    }
    sys_pwritev2(fd, iov, iovcnt, offset, 0)
}

pub fn sys_preadv2(
    fd: c_int,
    iov: *const IoVec,
    iovcnt: usize,
    offset: __kernel_off_t,
    flags: u32,
) -> AxResult<isize> {
    debug!("sys_preadv2 <= fd: {fd}, iovcnt: {iovcnt}, offset: {offset}, flags: {flags}");
    if flags != 0 {
        return Err(AxError::from(LinuxError::EOPNOTSUPP));
    }
    let f = positioned_file_from_fd(fd)?;
    let iov = IoVectorBuf::new(iov, iovcnt)?.into_io();
    if offset == -1 {
        f.inner().read(iov).map(|n| n as _)
    } else if offset < -1 {
        Err(AxError::InvalidInput)
    } else {
        f.inner().read_at(iov, offset as _).map(|n| n as _)
    }
}

pub fn sys_pwritev2(
    fd: c_int,
    iov: *const IoVec,
    iovcnt: usize,
    offset: __kernel_off_t,
    flags: u32,
) -> AxResult<isize> {
    debug!("sys_pwritev2 <= fd: {fd}, iovcnt: {iovcnt}, offset: {offset}, flags: {flags}");
    if flags != 0 {
        return Err(AxError::from(LinuxError::EOPNOTSUPP));
    }
    let f = positioned_file_from_fd(fd)?;
    let data = read_iov_to_vec(iov, iovcnt)?;
    if offset == -1 {
        check_memfd_write(&f)?;
        let len = limit_current_write_len(&f, data.len())?;
        let mut data = &data[..len];
        let written = f.inner().write(&mut data)?;
        if written > 0 {
            record_positioned_write(&f)?;
        }
        Ok(written as _)
    } else if offset < -1 {
        Err(AxError::InvalidInput)
    } else {
        let len = limit_positioned_write_len(&f, offset as u64, data.len())?;
        check_memfd_write_at(&f, offset as u64, len)?;
        let inner = f.inner();
        let mut data = &data[..len];
        if let Ok(file) = inner.access(FileFlags::APPEND) {
            let (written, _) = file.append(&mut data)?;
            if written > 0 {
                record_positioned_write(&f)?;
            }
            Ok(written as _)
        } else {
            let written = inner.write_at(&mut data, offset as _)?;
            if written > 0 {
                record_positioned_write(&f)?;
            }
            Ok(written as _)
        }
    }
}

enum SendFile {
    Direct(Arc<dyn FileLike>),
    Offset(Arc<File>, *mut u64),
}

impl SendFile {
    fn has_data(&self) -> bool {
        match self {
            SendFile::Direct(file) => file.poll(),
            SendFile::Offset(file, ..) => file.poll(),
        }
        .contains(IoEvents::IN)
    }

    fn read(&mut self, mut buf: &mut [u8]) -> AxResult<usize> {
        match self {
            SendFile::Direct(file) => file.read(&mut buf),
            SendFile::Offset(file, offset) => {
                let off = offset.vm_read()?;
                let bytes_read = file.inner().read_at(&mut buf, off)?;
                offset.vm_write(off + bytes_read as u64)?;
                Ok(bytes_read)
            }
        }
    }

    fn write(&mut self, mut buf: &[u8]) -> AxResult<usize> {
        match self {
            SendFile::Direct(file) => file.write(&mut buf),
            SendFile::Offset(file, offset) => {
                let off = offset.vm_read()?;
                let bytes_written = file.inner().write_at(buf, off)?;
                offset.vm_write(off + bytes_written as u64)?;
                Ok(bytes_written)
            }
        }
    }
}

fn do_send(mut src: SendFile, mut dst: SendFile, len: usize) -> AxResult<usize> {
    let mut buf = vec![0; 0x1000];
    let mut total_written = 0;
    let mut remaining = len;

    while remaining > 0 {
        if total_written > 0 && !src.has_data() {
            break;
        }
        let to_read = buf.len().min(remaining);
        let bytes_read = match src.read(&mut buf[..to_read]) {
            Ok(n) => n,
            Err(AxError::WouldBlock) if total_written > 0 => break,
            Err(e) => return Err(e),
        };
        if bytes_read == 0 {
            break;
        }

        let bytes_written = dst.write(&buf[..bytes_read])?;
        if bytes_written < bytes_read {
            break;
        }

        total_written += bytes_written;
        remaining -= bytes_written;
    }

    Ok(total_written)
}

enum SpliceEndpoint {
    File(Arc<File>),
    Pipe(Arc<Pipe>),
    #[cfg(feature = "net")]
    Socket(Arc<Socket>),
}

fn splice_endpoint(fd: c_int) -> AxResult<SpliceEndpoint> {
    let file = get_file_like(fd)?;
    if file.downcast_ref::<Pipe>().is_some() {
        return Pipe::from_fd(fd).map(SpliceEndpoint::Pipe);
    }
    if file.downcast_ref::<File>().is_some() {
        return File::from_fd(fd).map(SpliceEndpoint::File);
    }
    #[cfg(feature = "net")]
    if file.downcast_ref::<Socket>().is_some() {
        return Socket::from_fd(fd).map(SpliceEndpoint::Socket);
    }
    Err(AxError::InvalidInput)
}

fn splice_offset(offset: *mut i64) -> AxResult<*mut u64> {
    if offset.vm_read()? < 0 {
        return Err(AxError::InvalidInput);
    }
    Ok(offset.cast())
}

fn splice_src(endpoint: SpliceEndpoint, offset: *mut i64) -> AxResult<SendFile> {
    match endpoint {
        SpliceEndpoint::Pipe(pipe) => {
            if !offset.is_null() {
                return Err(AxError::from(LinuxError::ESPIPE));
            }
            if !pipe.is_read() {
                return Err(AxError::BadFileDescriptor);
            }
            Ok(SendFile::Direct(pipe))
        }
        SpliceEndpoint::File(file) => {
            if file.inner().is_path() {
                return Err(AxError::InvalidInput);
            }
            file.inner().access(FileFlags::READ)?;
            if offset.is_null() {
                Ok(SendFile::Direct(file))
            } else {
                Ok(SendFile::Offset(file, splice_offset(offset)?))
            }
        }
        #[cfg(feature = "net")]
        SpliceEndpoint::Socket(socket) => {
            if !offset.is_null() {
                return Err(AxError::InvalidInput);
            }
            Ok(SendFile::Direct(socket))
        }
    }
}

fn splice_dst(endpoint: SpliceEndpoint, offset: *mut i64) -> AxResult<SendFile> {
    match endpoint {
        SpliceEndpoint::Pipe(pipe) => {
            if !offset.is_null() {
                return Err(AxError::from(LinuxError::ESPIPE));
            }
            if !pipe.is_write() {
                return Err(AxError::BadFileDescriptor);
            }
            Ok(SendFile::Direct(pipe))
        }
        SpliceEndpoint::File(file) => {
            if file.inner().is_path() || file.inner().access(FileFlags::APPEND).is_ok() {
                return Err(AxError::InvalidInput);
            }
            file.inner().access(FileFlags::WRITE)?;
            if offset.is_null() {
                Ok(SendFile::Direct(file))
            } else {
                Ok(SendFile::Offset(file, splice_offset(offset)?))
            }
        }
        #[cfg(feature = "net")]
        SpliceEndpoint::Socket(socket) => {
            if !offset.is_null() {
                return Err(AxError::InvalidInput);
            }
            Ok(SendFile::Direct(socket))
        }
    }
}

fn splice_error(err: AxError) -> AxError {
    if err == AxError::NotConnected {
        AxError::InvalidInput
    } else {
        err
    }
}

fn splice_is_pipe(endpoint: &SpliceEndpoint) -> bool {
    matches!(endpoint, SpliceEndpoint::Pipe(_))
}

pub fn sys_sendfile(out_fd: c_int, in_fd: c_int, offset: *mut u64, len: usize) -> AxResult<isize> {
    debug!(
        "sys_sendfile <= out_fd: {}, in_fd: {}, offset: {}, len: {}",
        out_fd,
        in_fd,
        !offset.is_null(),
        len
    );

    if let Ok(file) = File::from_fd(in_fd) {
        file.inner().access(FileFlags::READ)?;
    }
    if let Ok(file) = File::from_fd(out_fd) {
        file.inner().access(FileFlags::WRITE)?;
    }

    let src = if !offset.is_null() {
        let off = offset.vm_read()?;
        if off > i64::MAX as u64 {
            return Err(AxError::InvalidInput);
        }
        offset.vm_write(off)?;
        SendFile::Offset(File::from_fd(in_fd)?, offset)
    } else {
        SendFile::Direct(get_file_like(in_fd)?)
    };

    let dst = SendFile::Direct(get_file_like(out_fd)?);

    do_send(src, dst, len).map(|n| n as _)
}

pub fn sys_copy_file_range(
    fd_in: c_int,
    off_in: *mut u64,
    fd_out: c_int,
    off_out: *mut u64,
    len: usize,
    _flags: u32,
) -> AxResult<isize> {
    debug!(
        "sys_copy_file_range <= fd_in: {}, off_in: {}, fd_out: {}, off_out: {}, len: {}, flags: {}",
        fd_in,
        !off_in.is_null(),
        fd_out,
        !off_out.is_null(),
        len,
        _flags
    );

    if _flags != 0 {
        return Err(AxError::InvalidInput);
    }
    if len > i64::MAX as usize {
        return Err(AxError::from(LinuxError::EOVERFLOW));
    }
    if Pipe::from_fd(fd_in).is_ok() || Pipe::from_fd(fd_out).is_ok() {
        return Err(AxError::InvalidInput);
    }

    let src_file = File::from_fd(fd_in)?;
    if Directory::from_fd(fd_out).is_ok() {
        return Err(AxError::IsADirectory);
    }
    let dst_file = File::from_fd(fd_out).map_err(|err| match err {
        AxError::InvalidInput => AxError::InvalidInput,
        err => err,
    })?;

    src_file.inner().access(FileFlags::READ)?;
    if dst_file.inner().access(FileFlags::APPEND).is_ok() {
        return Err(AxError::BadFileDescriptor);
    }
    dst_file.inner().access(FileFlags::WRITE)?;

    let src_meta = src_file.inner().location().metadata()?;
    let dst_meta = dst_file.inner().location().metadata()?;
    if src_meta.node_type != NodeType::RegularFile || dst_meta.node_type != NodeType::RegularFile {
        return Err(AxError::InvalidInput);
    }

    if let Ok(path) = dst_file.inner().location().absolute_path()
        && inode_flags(path.as_str()) & FS_IMMUTABLE_FL != 0
    {
        return Err(AxError::from(LinuxError::EPERM));
    }

    let src_off = if off_in.is_null() {
        0
    } else {
        off_in.vm_read()?
    };
    let dst_off = if off_out.is_null() {
        0
    } else {
        off_out.vm_read()?
    };
    if dst_off > i64::MAX as u64
        || dst_off
            .checked_add(len as u64)
            .is_none_or(|end| end > i64::MAX as u64)
    {
        return Err(AxError::from(LinuxError::EFBIG));
    }

    if src_meta.device == dst_meta.device && src_meta.inode == dst_meta.inode {
        let src_end = src_off.saturating_add(len as u64);
        let dst_end = dst_off.saturating_add(len as u64);
        if src_off < dst_end && dst_off < src_end {
            return Err(AxError::InvalidInput);
        }
    }

    if src_meta.device != dst_meta.device {
        return Err(AxError::from(LinuxError::EXDEV));
    }

    let src = if !off_in.is_null() {
        SendFile::Offset(src_file, off_in)
    } else {
        SendFile::Direct(src_file)
    };

    let dst = if !off_out.is_null() {
        SendFile::Offset(dst_file.clone(), off_out)
    } else {
        SendFile::Direct(dst_file.clone())
    };

    let written = do_send(src, dst, len)?;
    if written > 0 {
        record_positioned_write(&dst_file)?;
        notify_inotify_file(dst_file.as_ref(), IN_MODIFY);
        let dst_file_like: Arc<dyn FileLike> = dst_file;
        notify_fanotify_modify(&dst_file_like);
    }
    Ok(written as _)
}

pub fn sys_splice(
    fd_in: c_int,
    off_in: *mut i64,
    fd_out: c_int,
    off_out: *mut i64,
    len: usize,
    _flags: u32,
) -> AxResult<isize> {
    debug!(
        "sys_splice <= fd_in: {}, off_in: {}, fd_out: {}, off_out: {}, len: {}, flags: {}",
        fd_in,
        !off_in.is_null(),
        fd_out,
        !off_out.is_null(),
        len,
        _flags
    );

    if DummyFd::from_fd(fd_in).is_ok() || DummyFd::from_fd(fd_out).is_ok() {
        return Err(AxError::BadFileDescriptor);
    }

    let src_endpoint = splice_endpoint(fd_in)?;
    let dst_endpoint = splice_endpoint(fd_out)?;

    let has_pipe = splice_is_pipe(&src_endpoint) || splice_is_pipe(&dst_endpoint);

    if !has_pipe {
        return Err(AxError::InvalidInput);
    }

    let src = splice_src(src_endpoint, off_in)?;
    let dst = splice_dst(dst_endpoint, off_out)?;

    do_send(src, dst, len).map_err(splice_error).map(|n| n as _)
}

pub fn sys_vmsplice(fd: c_int, iov: *const IoVec, nr_segs: usize, flags: u32) -> AxResult<isize> {
    debug!("sys_vmsplice <= fd: {fd}, nr_segs: {nr_segs}, flags: {flags}");

    const ALLOWED_FLAGS: u32 = SPLICE_F_MOVE | SPLICE_F_NONBLOCK | SPLICE_F_MORE | SPLICE_F_GIFT;
    if flags & !ALLOWED_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }

    let pipe = Pipe::from_fd(fd).map_err(|err| match err {
        AxError::InvalidInput => AxError::BadFileDescriptor,
        err => err,
    })?;
    let mut iov = IoVectorBuf::new(iov, nr_segs)?.into_io();

    let old_nonblocking = pipe.nonblocking();
    if flags & SPLICE_F_NONBLOCK != 0 && !old_nonblocking {
        pipe.set_nonblocking(true)?;
    }

    let result = if pipe.is_read() {
        pipe.read(&mut iov)
    } else if flags & SPLICE_F_NONBLOCK == 0 {
        match pipe.write_once(&mut iov) {
            Err(AxError::WouldBlock) => pipe.write(&mut iov),
            result => result,
        }
    } else {
        pipe.write(&mut iov)
    };

    if flags & SPLICE_F_NONBLOCK != 0 && !old_nonblocking {
        pipe.set_nonblocking(false)?;
    }

    result.map(|n| n as _)
}
