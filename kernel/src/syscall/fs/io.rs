use alloc::{borrow::Cow, sync::Arc, vec};
use core::{
    ffi::{c_char, c_int},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, FileFlags, OpenOptions};
use axfs_ng_vfs::NodeType;
use axio::{Seek, SeekFrom};
use axpoll::{IoEvents, Pollable};
use axtask::current;
use linux_raw_sys::general::{__kernel_off_t, AT_FDCWD, O_ACCMODE, O_WRONLY};
use starry_vm::{VmMutPtr, VmPtr};
use syscalls::Sysno;

use super::fanotify::{
    check_fanotify_access_permission, notify_fanotify_access, notify_fanotify_modify,
    notify_fanotify_modify_path,
};
#[cfg(feature = "net")]
use crate::file::Socket;
use crate::{
    file::{File, FileLike, Pipe, get_file_like},
    mm::{IoVec, IoVectorBuf, UserConstPtr, VmBytes, VmBytesMut},
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

/// Read data from the file indicated by `fd`.
///
/// Return the read size if success.
pub fn sys_read(fd: i32, buf: *mut u8, len: usize) -> AxResult<isize> {
    debug!("sys_read <= fd: {fd}, buf: {buf:p}, len: {len}");
    let f = get_file_like(fd)?;
    if len > 0 {
        check_fanotify_access_permission(f.clone())?;
    }
    let read = f.read(&mut VmBytesMut::new(buf, len))?;
    if read > 0 {
        notify_fanotify_access(f);
    }
    Ok(read as _)
}

pub fn sys_readv(fd: i32, iov: *const IoVec, iovcnt: usize) -> AxResult<isize> {
    debug!("sys_readv <= fd: {fd}, iovcnt: {iovcnt}");
    let f = get_file_like(fd)?;
    let iov = IoVectorBuf::new(iov, iovcnt)?;
    if iovcnt > 0 {
        check_fanotify_access_permission(f.clone())?;
    }
    let read = f.read(&mut iov.into_io())?;
    if read > 0 {
        notify_fanotify_access(f);
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
    let written = f.write(&mut VmBytes::new(buf, len))?;
    if written > 0 {
        notify_fanotify_modify(f);
    }
    Ok(written as _)
}

pub fn sys_writev(fd: i32, iov: *const IoVec, iovcnt: usize) -> AxResult<isize> {
    debug!("sys_writev <= fd: {fd}, iovcnt: {iovcnt}");
    let f = get_file_like(fd)?;
    let written = f.write(&mut IoVectorBuf::new(iov, iovcnt)?.into_io())?;
    if written > 0 {
        notify_fanotify_modify(f);
    }
    Ok(written as _)
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
    if length < 0 {
        return Err(AxError::InvalidInput);
    }
    let file = OpenOptions::new()
        .write(true)
        .open(&FS_CONTEXT.lock(), path)?
        .into_file()?;
    file.access(FileFlags::WRITE)?.set_len(length as _)?;
    notify_fanotify_modify_path(AT_FDCWD, path);
    Ok(0)
}

pub fn sys_ftruncate(fd: c_int, length: __kernel_off_t) -> AxResult<isize> {
    debug!("sys_ftruncate <= {fd} {length}");
    let f = File::from_fd(fd)?;
    f.inner().access(FileFlags::WRITE)?.set_len(length as _)?;
    notify_fanotify_modify(f);
    Ok(0)
}

pub fn sys_fallocate(
    fd: c_int,
    mode: u32,
    offset: __kernel_off_t,
    len: __kernel_off_t,
) -> AxResult<isize> {
    debug!("sys_fallocate <= fd: {fd}, mode: {mode}, offset: {offset}, len: {len}");
    if mode != 0 {
        return Err(AxError::InvalidInput);
    }
    let f = File::from_fd(fd)?;
    let inner = f.inner();
    let file = inner.access(FileFlags::WRITE)?;
    file.set_len(file.location().len()?.max(offset as u64 + len as u64))?;
    Ok(0)
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

pub fn sys_fadvise64(
    fd: c_int,
    offset: __kernel_off_t,
    len: __kernel_off_t,
    advice: u32,
) -> AxResult<isize> {
    debug!("sys_fadvise64 <= fd: {fd}, offset: {offset}, len: {len}, advice: {advice}");
    if Pipe::from_fd(fd).is_ok() {
        return Err(AxError::BrokenPipe);
    }
    if advice > 5 {
        return Err(AxError::InvalidInput);
    }
    Ok(0)
}

pub fn sys_readahead(fd: c_int, offset: __kernel_off_t, _count: usize) -> AxResult<isize> {
    debug!("sys_readahead <= fd: {fd}, offset: {offset}");
    if offset < 0 {
        return Err(AxError::InvalidInput);
    }

    let file_like = get_file_like(fd)?;
    if file_like.path().as_ref() == "anon_inode:[pidfd]" {
        return Ok(0);
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
    if len == 0 {
        return Ok(0);
    }
    let inner = f.inner();
    let write = if let Ok(file) = inner.access(FileFlags::APPEND) {
        file.append(VmBytes::new(buf, len))?.0
    } else {
        inner.write_at(VmBytes::new(buf, len), offset as _)?
    };
    Ok(write as _)
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
    let iov = IoVectorBuf::new(iov, iovcnt)?.into_io();
    if offset == -1 {
        f.inner().write(iov).map(|n| n as _)
    } else if offset < -1 {
        Err(AxError::InvalidInput)
    } else {
        let inner = f.inner();
        if let Ok(file) = inner.access(FileFlags::APPEND) {
            file.append(iov).map(|(n, _)| n as _)
        } else {
            inner.write_at(iov, offset as _).map(|n| n as _)
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

    // TODO: check flags
    // TODO: check both regular files
    // TODO: check same file and overlap
    if let (Ok(src), Ok(dst)) = (File::from_fd(fd_in), File::from_fd(fd_out)) {
        src.inner().access(FileFlags::READ)?;
        dst.inner().access(FileFlags::WRITE)?;

        if src.inner().location().metadata()?.device != dst.inner().location().metadata()?.device {
            return Err(AxError::from(LinuxError::EXDEV));
        }
    }

    let src = if !off_in.is_null() {
        SendFile::Offset(File::from_fd(fd_in)?, off_in)
    } else {
        SendFile::Direct(get_file_like(fd_in)?)
    };

    let dst = if !off_out.is_null() {
        SendFile::Offset(File::from_fd(fd_out)?, off_out)
    } else {
        SendFile::Direct(get_file_like(fd_out)?)
    };

    do_send(src, dst, len).map(|n| n as _)
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
