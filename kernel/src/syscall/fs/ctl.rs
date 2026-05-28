use alloc::{ffi::CString, string::ToString, vec, vec::Vec};
use core::{
    ffi::{c_char, c_int},
    mem::offset_of,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use axerrno::{AxError, AxResult};
use axfs::{FS_CONTEXT, FsContext};
use axfs_ng_vfs::{DeviceId, Location, MetadataUpdate, NodePermission, NodeType, path::Path};
use axhal::time::realtime_time;
use axtask::current;
use linux_raw_sys::{
    general::*,
    ioctl::{
        FIONBIO, FS_IOC_GETFLAGS, FS_IOC_SETFLAGS, TCGETA, TCGETS, TCGETS2, TIOCGPTPEER, TIOCGWINSZ,
    },
};
use starry_vm::{VmMutPtr, VmPtr, vm_write_slice};

use super::{
    fanotify::{
        commit_fanotify_path_alias, fanotify_path_is_dir, fanotify_prepared_event_is_dir,
        notify_fanotify_create, notify_fanotify_delete, notify_fanotify_move_from,
        notify_fanotify_move_to, notify_fanotify_prepared_event, prepare_fanotify_delete_event,
        prepare_fanotify_delete_self_event, prepare_fanotify_move_from_event,
        prepare_fanotify_move_self_event, prepare_fanotify_path_alias,
        prepare_fanotify_rename_event,
    },
    inotify::{
        next_inotify_cookie, notify_inotify_dirent, notify_inotify_path, notify_inotify_target,
        prepare_inotify_target,
    },
};
use crate::{
    file::{
        Directory, File, FileLike, get_file_like, inode_flags, register_special_node, resolve_at,
        set_inode_flags, with_fs,
    },
    mm::vm_load_string,
    task::AsThread,
    time::TimeValueLike,
};

static PROTECTED_HARDLINKS: AtomicBool = AtomicBool::new(true);
static PROTECTED_SYMLINKS: AtomicBool = AtomicBool::new(true);

pub fn protected_hardlinks_enabled() -> bool {
    PROTECTED_HARDLINKS.load(Ordering::Relaxed)
}

pub fn set_protected_hardlinks(enabled: bool) {
    PROTECTED_HARDLINKS.store(enabled, Ordering::Relaxed);
}

pub fn protected_symlinks_enabled() -> bool {
    PROTECTED_SYMLINKS.load(Ordering::Relaxed)
}

pub fn set_protected_symlinks(enabled: bool) {
    PROTECTED_SYMLINKS.store(enabled, Ordering::Relaxed);
}

/// The ioctl() system call manipulates the underlying device parameters
/// of special files.
pub fn sys_ioctl(fd: i32, cmd: u32, arg: usize) -> AxResult<isize> {
    debug!("sys_ioctl <= fd: {fd}, cmd: {cmd}, arg: {arg}");
    let f = get_file_like(fd)?;
    if cmd == FIONBIO {
        let val = (arg as *const u8).vm_read()?;
        if val != 0 && val != 1 {
            return Err(AxError::InvalidInput);
        }
        f.set_nonblocking(val != 0)?;
        return Ok(0);
    }
    if cmd == FS_IOC_GETFLAGS {
        (arg as *mut u32).vm_write(inode_flags(f.path().as_ref()))?;
        return Ok(0);
    }
    if cmd == FS_IOC_SETFLAGS {
        let flags = (arg as *const u32).vm_read()?;
        set_inode_flags(f.path().to_string(), flags);
        return Ok(0);
    }
    f.ioctl(cmd, arg)
        .map(|result| result as isize)
        .inspect_err(|err| {
            if *err == AxError::NotATty {
                // libc/test harnesses commonly probe stdio fds with terminal
                // ioctls. Returning ENOTTY is correct; keep the log quiet.
                if matches!(cmd, TCGETS | TCGETA | TCGETS2 | TIOCGWINSZ | TIOCGPTPEER)
                    || cmd == 9_999_999
                {
                    return;
                }
                warn!("Unsupported ioctl command: {cmd} for fd: {fd}");
            }
        })
}

pub fn sys_chdir(path: *const c_char) -> AxResult<isize> {
    let path = vm_load_string(path)?;
    debug!("sys_chdir <= path: {path}");

    let mut fs = FS_CONTEXT.lock();
    let entry = fs.resolve(path)?;
    check_chdir_access(&entry)?;
    fs.set_current_dir(entry)?;
    Ok(0)
}

pub fn sys_fchdir(dirfd: i32) -> AxResult<isize> {
    debug!("sys_fchdir <= dirfd: {dirfd}");

    let entry = with_fs(dirfd, |fs| Ok(fs.current_dir().clone()))?;
    check_chdir_access(&entry)?;
    FS_CONTEXT.lock().set_current_dir(entry)?;
    Ok(0)
}

fn check_chdir_access(entry: &Location) -> AxResult<()> {
    let metadata = entry.metadata()?;
    if metadata.node_type != NodeType::Directory {
        return Err(AxError::NotADirectory);
    }

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let euid = proc_data.euid();
    if euid == 0 {
        return Ok(());
    }

    let egid = proc_data.egid();
    let shift = if euid == metadata.uid {
        6
    } else if egid == metadata.gid {
        3
    } else {
        0
    };
    if metadata.mode.bits() as u32 & (0o1 << shift) == 0 {
        return Err(AxError::PermissionDenied);
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
pub fn sys_mkdir(path: *const c_char, mode: u32) -> AxResult<isize> {
    sys_mkdirat(AT_FDCWD, path, mode)
}

pub fn sys_chroot(path: *const c_char) -> AxResult<isize> {
    let path = vm_load_string(path)?;
    debug!("sys_chroot <= path: {path}");

    let mut fs = FS_CONTEXT.lock();
    let loc = fs.resolve(path)?;
    if loc.node_type() != NodeType::Directory {
        return Err(AxError::NotADirectory);
    }
    *fs = FsContext::new(loc);
    Ok(0)
}

pub fn sys_mkdirat(dirfd: i32, path: *const c_char, mode: u32) -> AxResult<isize> {
    let path = vm_load_string(path)?;
    debug!("sys_mkdirat <= dirfd: {dirfd}, path: {path}, mode: {mode}");
    if path.is_empty() {
        return Err(AxError::NotFound);
    }

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let mode = mode & !proc_data.umask();
    let euid = proc_data.euid();
    let egid = proc_data.egid();

    with_fs(dirfd, |fs| {
        let path = Path::new(path.as_str());
        if fs.resolve_no_follow(path).is_ok() {
            return Err(AxError::AlreadyExists);
        }

        let (dir, name) = fs.resolve_nonexistent(path)?;
        let dir_stat = dir.metadata()?;
        if dir.mount_flags() & MS_RDONLY != 0 {
            return Err(AxError::ReadOnlyFilesystem);
        }
        if euid != 0
            && !can_write_dir(
                dir_stat.mode.bits() as u32,
                dir_stat.uid,
                dir_stat.gid,
                euid,
                egid,
            )
        {
            return Err(AxError::PermissionDenied);
        }

        let mut permission = NodePermission::from_bits_truncate(mode as u16);
        let gid = if dir_stat.mode.contains(NodePermission::SET_GID) {
            permission.insert(NodePermission::SET_GID);
            dir_stat.gid
        } else {
            egid
        };

        let loc = dir.create(name, NodeType::Directory, permission)?;
        loc.update_metadata(MetadataUpdate {
            owner: Some((euid, gid)),
            mode: Some(permission),
            ..Default::default()
        })?;
        Ok(0)
    })?;
    notify_inotify_dirent(dirfd, &path, IN_CREATE, true, 0);
    notify_fanotify_create(dirfd, &path, true);
    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_mknod(path: *const c_char, mode: u32, dev: u64) -> AxResult<isize> {
    sys_mknodat(AT_FDCWD, path, mode, dev)
}

fn dev_id_from_raw(dev: u64) -> DeviceId {
    let major = ((dev >> 8) & 0x0000_0fff) | ((dev >> 32) & 0xffff_f000);
    let minor = (dev & 0x0000_00ff) | ((dev >> 12) & 0xffff_ff00);
    DeviceId::new(major as _, minor as _)
}

pub fn sys_mknodat(dirfd: i32, path: *const c_char, mode: u32, dev: u64) -> AxResult<isize> {
    let path = vm_load_string(path)?;
    debug!("sys_mknodat <= dirfd: {dirfd}, path: {path:?}, mode: {mode:#o}, dev: {dev:#x}");
    if path.is_empty() {
        return Err(AxError::NotFound);
    }

    let node_type = match mode & S_IFMT {
        0 | S_IFREG => NodeType::RegularFile,
        S_IFIFO => NodeType::Fifo,
        S_IFCHR => NodeType::CharacterDevice,
        S_IFBLK => NodeType::BlockDevice,
        S_IFSOCK => NodeType::Socket,
        _ => return Err(AxError::InvalidInput),
    };
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let mode = mode & !proc_data.umask();
    let permission = NodePermission::from_bits_truncate(mode as u16);
    let special = matches!(node_type, NodeType::CharacterDevice | NodeType::BlockDevice);
    let euid = proc_data.euid();
    let egid = proc_data.egid();

    if special && euid != 0 {
        return Err(AxError::OperationNotPermitted);
    }

    let loc = with_fs(dirfd, |fs| {
        let (dir, name) = fs.resolve_nonexistent(Path::new(path.as_str()))?;
        let dir_stat = dir.metadata()?;
        if dir.mount_flags() & MS_RDONLY != 0 {
            return Err(AxError::ReadOnlyFilesystem);
        }
        if euid != 0
            && !can_write_dir(
                dir_stat.mode.bits() as u32,
                dir_stat.uid,
                dir_stat.gid,
                euid,
                egid,
            )
        {
            return Err(AxError::PermissionDenied);
        }
        let gid = if dir_stat.mode.contains(NodePermission::SET_GID) {
            dir_stat.gid
        } else {
            egid
        };
        match dir.create(name, node_type, permission) {
            Ok(loc) => {
                loc.update_metadata(MetadataUpdate {
                    owner: Some((euid, gid)),
                    ..Default::default()
                })?;
                Ok(loc)
            }
            Err(_) if special => {
                let loc = dir.create(name, NodeType::RegularFile, permission)?;
                loc.update_metadata(MetadataUpdate {
                    owner: Some((euid, gid)),
                    ..Default::default()
                })?;
                Ok(loc)
            }
            Err(err) => Err(err.into()),
        }
    })?;

    if special && let Ok(path) = loc.absolute_path() {
        register_special_node(path.to_string(), mode, dev_id_from_raw(dev));
    }

    notify_inotify_dirent(dirfd, &path, IN_CREATE, false, 0);
    notify_fanotify_create(dirfd, &path, false);
    Ok(0)
}

fn can_write_dir(mode: u32, uid: u32, gid: u32, euid: u32, egid: u32) -> bool {
    let shift = if euid == uid {
        6
    } else if egid == gid {
        3
    } else {
        0
    };
    let required = 0o3 << shift;
    mode & required == required
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

fn check_protected_hardlink(target: &Location) -> AxResult<()> {
    if !protected_hardlinks_enabled() {
        return Ok(());
    }

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let euid = proc_data.euid();
    if euid == 0 {
        return Ok(());
    }

    let metadata = target.metadata()?;
    if euid == metadata.uid
        || can_write_file(
            metadata.mode.bits() as u32,
            metadata.uid,
            metadata.gid,
            euid,
            proc_data.egid(),
        )
    {
        Ok(())
    } else {
        Err(AxError::OperationNotPermitted)
    }
}

// Directory buffer for getdents64 syscall
struct DirBuffer {
    buf: Vec<u8>,
    offset: usize,
}

impl DirBuffer {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0; len],
            offset: 0,
        }
    }

    fn remaining_space(&self) -> usize {
        self.buf.len().saturating_sub(self.offset)
    }

    fn write_entry(&mut self, d_ino: u64, d_off: i64, d_type: NodeType, name: &[u8]) -> bool {
        const NAME_OFFSET: usize = offset_of!(linux_dirent64, d_name);

        let len = NAME_OFFSET + name.len() + 1;
        // alignment
        let len = len.next_multiple_of(align_of::<linux_dirent64>());
        if self.remaining_space() < len {
            return false;
        }

        // FIXME: safety
        unsafe {
            let entry_ptr = self.buf.as_mut_ptr().add(self.offset);
            entry_ptr.cast::<linux_dirent64>().write(linux_dirent64 {
                d_ino,
                d_off,
                d_reclen: len as _,
                d_type: d_type as _,
                d_name: Default::default(),
            });

            let name_ptr = entry_ptr.add(NAME_OFFSET);
            name_ptr.copy_from_nonoverlapping(name.as_ptr(), name.len());
            name_ptr.add(name.len()).write(0);
        }

        self.offset += len;
        true
    }
}

pub fn sys_getdents64(fd: i32, buf: *mut u8, len: usize) -> AxResult<isize> {
    debug!("sys_getdents64 <= fd: {fd}, buf: {buf:?}, len: {len}");

    let mut buffer = DirBuffer::new(len);

    let dir = Directory::from_fd(fd)?;
    let mut dir_offset = dir.offset.lock();

    let mut has_remaining = false;

    dir.inner()
        .read_dir(*dir_offset, &mut |name: &str, ino, node_type, offset| {
            has_remaining = true;
            if !buffer.write_entry(ino, offset as _, node_type, name.as_bytes()) {
                return false;
            }
            *dir_offset = offset;
            true
        })?;

    if has_remaining && buffer.offset == 0 {
        return Err(AxError::InvalidInput);
    }

    vm_write_slice(buf, &buffer.buf)?;

    Ok(buffer.offset as _)
}

/// create a link from new_path to old_path
/// old_path: old file path
/// new_path: new file path
/// flags: link flags
/// return value: return 0 when success, else return -1.
pub fn sys_linkat(
    old_dirfd: c_int,
    old_path: *const c_char,
    new_dirfd: c_int,
    new_path: *const c_char,
    flags: u32,
) -> AxResult<isize> {
    let old_path = old_path.nullable().map(vm_load_string).transpose()?;
    let new_path = vm_load_string(new_path)?;
    debug!(
        "sys_linkat <= old_dirfd: {old_dirfd}, old_path: {old_path:?}, new_dirfd: {new_dirfd}, \
         new_path: {new_path}, flags: {flags}"
    );

    if flags != 0 {
        warn!("Unsupported flags: {flags}");
    }

    let old = resolve_at(old_dirfd, old_path.as_deref(), flags)?
        .into_file()
        .ok_or(AxError::BadFileDescriptor)?;
    if old.is_dir() {
        return Err(AxError::OperationNotPermitted);
    }
    check_protected_hardlink(&old)?;
    let (new_dir, new_name) =
        with_fs(new_dirfd, |fs| fs.resolve_nonexistent(Path::new(&new_path)))?;

    new_dir.link(new_name, &old)?;
    notify_inotify_dirent(new_dirfd, &new_path, IN_CREATE, false, 0);
    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_link(old_path: *const c_char, new_path: *const c_char) -> AxResult<isize> {
    sys_linkat(AT_FDCWD, old_path, AT_FDCWD, new_path, 0)
}

/// remove link of specific file (can be used to delete file)
/// dir_fd: the directory of link to be removed
/// path: the name of link to be removed
/// flags: can be 0 or AT_REMOVEDIR
/// return 0 when success, else return -1
pub fn sys_unlinkat(dirfd: i32, path: *const c_char, flags: usize) -> AxResult<isize> {
    let path = vm_load_string(path)?;

    debug!("sys_unlinkat <= dirfd: {dirfd}, path: {path:?}, flags: {flags}");

    let self_event = prepare_fanotify_delete_self_event(dirfd, &path);
    let is_dir = self_event
        .as_ref()
        .map(fanotify_prepared_event_is_dir)
        .unwrap_or(flags == AT_REMOVEDIR as _);
    let dirent_event = prepare_fanotify_delete_event(dirfd, &path, is_dir);
    let inotify_target = prepare_inotify_target(dirfd, &path);
    with_fs(dirfd, |fs| {
        if flags == AT_REMOVEDIR as _ {
            fs.remove_dir(path.as_str())?;
        } else {
            fs.remove_file(path.as_str())?;
        }
        Ok(0)
    })?;
    let mut self_event = self_event;
    if is_dir {
        if let Some(event) = self_event.take() {
            notify_fanotify_prepared_event(event);
        }
    }
    if !is_dir && let Some(target) = inotify_target {
        notify_inotify_target(target, IN_ATTRIB);
    }
    if let Some(target) = inotify_target {
        notify_inotify_target(target, IN_DELETE_SELF);
    }
    if let Some(event) = dirent_event {
        notify_fanotify_prepared_event(event);
    } else {
        notify_fanotify_delete(dirfd, &path, is_dir);
    }
    notify_inotify_dirent(dirfd, &path, IN_DELETE, is_dir, 0);
    if let Some(event) = self_event {
        notify_fanotify_prepared_event(event);
    }
    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_rmdir(path: *const c_char) -> AxResult<isize> {
    sys_unlinkat(AT_FDCWD, path, AT_REMOVEDIR as _)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_unlink(path: *const c_char) -> AxResult<isize> {
    sys_unlinkat(AT_FDCWD, path, 0)
}

pub fn sys_getcwd(buf: *mut u8, size: isize) -> AxResult<isize> {
    let size: usize = size.try_into().map_err(|_| AxError::BadAddress)?;
    if buf.is_null() {
        return Ok(0);
    }

    let cwd = FS_CONTEXT.lock().current_dir().absolute_path()?;
    debug!("sys_getcwd => cwd: {cwd}");

    let cwd = CString::new(cwd.as_str()).map_err(|_| AxError::InvalidInput)?;
    let cwd = cwd.as_bytes_with_nul();

    if cwd.len() <= size {
        vm_write_slice(buf, cwd)?;
        Ok(cwd.len() as _)
    } else {
        Err(AxError::OutOfRange)
    }
}

#[cfg(target_arch = "x86_64")]
pub fn sys_symlink(target: *const c_char, linkpath: *const c_char) -> AxResult<isize> {
    sys_symlinkat(target, AT_FDCWD, linkpath)
}

pub fn sys_symlinkat(
    target: *const c_char,
    new_dirfd: i32,
    linkpath: *const c_char,
) -> AxResult<isize> {
    let target = vm_load_string(target)?;
    let linkpath = vm_load_string(linkpath)?;
    debug!("sys_symlinkat <= target: {target:?}, new_dirfd: {new_dirfd}, linkpath: {linkpath:?}");

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let euid = proc_data.euid();
    let egid = proc_data.egid();
    with_fs(new_dirfd, |fs| {
        let loc = fs.symlink(target, &linkpath)?;
        loc.update_metadata(MetadataUpdate {
            owner: Some((euid, egid)),
            mode: Some(NodePermission::from_bits_truncate(0o777)),
            ..Default::default()
        })?;
        Ok(0)
    })?;
    notify_inotify_dirent(new_dirfd, &linkpath, IN_CREATE, false, 0);
    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_readlink(path: *const c_char, buf: *mut u8, size: usize) -> AxResult<isize> {
    sys_readlinkat(AT_FDCWD, path, buf, size)
}

pub fn sys_readlinkat(
    dirfd: i32,
    path: *const c_char,
    buf: *mut u8,
    size: usize,
) -> AxResult<isize> {
    let path = vm_load_string(path)?;

    debug!("sys_readlinkat <= dirfd: {dirfd}, path: {path:?}");

    with_fs(dirfd, |fs| {
        let entry = fs.resolve_no_follow(path)?;
        let link = entry.read_link()?;
        let read = size.min(link.len());
        vm_write_slice(buf, &link.as_bytes()[..read])?;
        Ok(read as isize)
    })
}

#[cfg(target_arch = "x86_64")]
pub fn sys_chown(path: *const c_char, uid: i32, gid: i32) -> AxResult<isize> {
    sys_fchownat(AT_FDCWD, path, uid, gid, 0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_lchown(path: *const c_char, uid: i32, gid: i32) -> AxResult<isize> {
    use linux_raw_sys::general::AT_SYMLINK_NOFOLLOW;
    sys_fchownat(AT_FDCWD, path, uid, gid, AT_SYMLINK_NOFOLLOW)
}

pub fn sys_fchown(fd: i32, uid: i32, gid: i32) -> AxResult<isize> {
    sys_fchownat(fd, core::ptr::null(), uid, gid, AT_EMPTY_PATH)
}

pub fn sys_fchownat(
    dirfd: i32,
    path: *const c_char,
    uid: i32,
    gid: i32,
    flags: u32,
) -> AxResult<isize> {
    let path = path.nullable().map(vm_load_string).transpose()?;
    let loc = resolve_at(dirfd, path.as_deref(), flags)?
        .into_file()
        .ok_or(AxError::BadFileDescriptor)?;
    let meta = loc.metadata()?;

    let mut mode = meta.mode;
    // chown always clears the setuid bits
    mode.remove(NodePermission::SET_UID);
    // chown also removes the setgid bits if group-executable
    if mode.contains(NodePermission::GROUP_EXEC) {
        mode.remove(NodePermission::SET_GID);
    }

    let uid = if uid == -1 { meta.uid } else { uid as _ };
    let gid = if gid == -1 { meta.gid } else { gid as _ };
    loc.update_metadata(MetadataUpdate {
        owner: Some((uid, gid)),
        mode: Some(mode),
        ..Default::default()
    })?;
    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_chmod(path: *const c_char, mode: u32) -> AxResult<isize> {
    sys_fchmodat(AT_FDCWD, path, mode, 0)
}

pub fn sys_fchmod(fd: i32, mode: u32) -> AxResult<isize> {
    sys_fchmodat(fd, core::ptr::null(), mode, AT_EMPTY_PATH)
}

pub fn sys_fchmodat(dirfd: i32, path: *const c_char, mode: u32, flags: u32) -> AxResult<isize> {
    let path = path.nullable().map(vm_load_string).transpose()?;
    resolve_at(dirfd, path.as_deref(), flags)?
        .into_file()
        .ok_or(AxError::BadFileDescriptor)?
        .update_metadata(MetadataUpdate {
            mode: Some(NodePermission::from_bits_truncate(mode as u16)),
            ..Default::default()
        })?;
    if let Some(path) = path.as_deref() {
        notify_inotify_path(dirfd, path, IN_ATTRIB);
    }
    Ok(0)
}

#[derive(Clone, Copy)]
enum UtimeSpec {
    Omit,
    Now,
    Set(Duration),
}

impl UtimeSpec {
    fn duration(self) -> Option<Duration> {
        match self {
            Self::Omit => None,
            Self::Now => Some(realtime_time()),
            Self::Set(time) => Some(time),
        }
    }

    fn is_omit(self) -> bool {
        matches!(self, Self::Omit)
    }

    fn is_now(self) -> bool {
        matches!(self, Self::Now)
    }
}

fn update_times(
    dirfd: i32,
    path: *const c_char,
    atime: UtimeSpec,
    mtime: UtimeSpec,
    flags: u32,
) -> AxResult<()> {
    let path = path.nullable().map(vm_load_string).transpose()?;
    let loc = resolve_at(dirfd, path.as_deref(), flags)?
        .into_file()
        .ok_or(AxError::BadFileDescriptor)?;

    if let Ok(path) = loc.absolute_path() {
        let inode_flags = inode_flags(path.as_str());
        let omit_both = atime.is_omit() && mtime.is_omit();
        let now_both = atime.is_now() && mtime.is_now();
        if inode_flags & FS_IMMUTABLE_FL != 0 && !omit_both {
            return Err(AxError::OperationNotPermitted);
        }
        if inode_flags & FS_APPEND_FL != 0 && !(omit_both || now_both) {
            return Err(AxError::OperationNotPermitted);
        }
    }

    loc.update_metadata(MetadataUpdate {
        atime: atime.duration(),
        mtime: mtime.duration(),
        ..Default::default()
    })?;
    if let Some(path) = path.as_deref() {
        notify_inotify_path(dirfd, path, IN_ATTRIB);
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct utimbuf {
    actime: linux_raw_sys::general::__kernel_old_time_t,
    modtime: linux_raw_sys::general::__kernel_old_time_t,
}

#[cfg(target_arch = "x86_64")]
pub fn sys_utime(path: *const c_char, times: *const utimbuf) -> AxResult<isize> {
    let (atime, mtime) = if let Some(times) = times.nullable() {
        // FIXME: AnyBitPattern
        let times = unsafe { times.vm_read_uninit()?.assume_init() };
        (
            UtimeSpec::Set(Duration::from_secs(times.actime as _)),
            UtimeSpec::Set(Duration::from_secs(times.modtime as _)),
        )
    } else {
        (UtimeSpec::Now, UtimeSpec::Now)
    };
    update_times(AT_FDCWD, path, atime, mtime, 0)?;
    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_utimes(
    path: *const c_char,
    times: *const [linux_raw_sys::general::timeval; 2],
) -> AxResult<isize> {
    let (atime, mtime) = if let Some(times) = times.nullable() {
        // FIXME: AnyBitPattern
        let [atime, mtime] = unsafe { times.vm_read_uninit()?.assume_init() };
        (
            UtimeSpec::Set(atime.try_into_time_value()?),
            UtimeSpec::Set(mtime.try_into_time_value()?),
        )
    } else {
        (UtimeSpec::Now, UtimeSpec::Now)
    };
    update_times(AT_FDCWD, path, atime, mtime, 0)?;
    Ok(0)
}

pub fn sys_utimensat(
    dirfd: i32,
    path: *const c_char,
    times: *const [timespec; 2],
    mut flags: u32,
) -> AxResult<isize> {
    const VALID_FLAGS: u32 = AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH;
    if flags & !VALID_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }
    if path.is_null() {
        if flags & AT_SYMLINK_NOFOLLOW != 0 {
            return Err(AxError::InvalidInput);
        }
        flags |= AT_EMPTY_PATH;
    }
    fn utime_to_spec(time: &timespec) -> AxResult<UtimeSpec> {
        match time.tv_nsec {
            val if val == UTIME_OMIT as _ => Ok(UtimeSpec::Omit),
            val if val == UTIME_NOW as _ => Ok(UtimeSpec::Now),
            _ => Ok(UtimeSpec::Set(time.try_into_time_value()?)),
        }
    }

    let (atime, mtime) = if let Some(times) = times.nullable() {
        // FIXME: AnyBitPattern
        let [atime, mtime] = unsafe { times.vm_read_uninit()?.assume_init() };
        (utime_to_spec(&atime)?, utime_to_spec(&mtime)?)
    } else {
        (UtimeSpec::Now, UtimeSpec::Now)
    };
    if atime.is_omit() && mtime.is_omit() {
        return Ok(0);
    }

    update_times(dirfd, path, atime, mtime, flags)?;
    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_rename(old_path: *const c_char, new_path: *const c_char) -> AxResult<isize> {
    sys_renameat(AT_FDCWD, old_path, AT_FDCWD, new_path)
}

#[cfg(not(target_arch = "riscv64"))]
pub fn sys_renameat(
    old_dirfd: i32,
    old_path: *const c_char,
    new_dirfd: i32,
    new_path: *const c_char,
) -> AxResult<isize> {
    sys_renameat2(old_dirfd, old_path, new_dirfd, new_path, 0)
}

pub fn sys_renameat2(
    old_dirfd: i32,
    old_path: *const c_char,
    new_dirfd: i32,
    new_path: *const c_char,
    flags: u32,
) -> AxResult<isize> {
    let old_path = vm_load_string(old_path)?;
    let new_path = vm_load_string(new_path)?;
    debug!(
        "sys_renameat2 <= old_dirfd: {old_dirfd}, old_path: {old_path:?}, new_dirfd: {new_dirfd}, \
         new_path: {new_path}, flags: {flags}"
    );

    let (old_dir, old_name) = with_fs(old_dirfd, |fs| fs.resolve_parent(Path::new(&old_path)))?;
    let (new_dir, new_name) =
        with_fs(new_dirfd, |fs| fs.resolve_nonexistent(Path::new(&new_path)))?;
    let is_dir = fanotify_path_is_dir(old_dirfd, &old_path).unwrap_or(false);
    let move_from_event = prepare_fanotify_move_from_event(old_dirfd, &old_path, is_dir);
    let move_self_event = prepare_fanotify_move_self_event(old_dirfd, &old_path);
    let rename_event =
        prepare_fanotify_rename_event(old_dirfd, &old_path, new_dirfd, &new_path, is_dir);
    let path_alias = prepare_fanotify_path_alias(old_dirfd, &old_path, new_dirfd, &new_path);
    let inotify_target = prepare_inotify_target(old_dirfd, &old_path);
    let cookie = next_inotify_cookie();

    old_dir.rename(&old_name, &new_dir, new_name)?;
    commit_fanotify_path_alias(path_alias);
    let mut move_self_event = move_self_event;
    if is_dir {
        if let Some(event) = move_self_event.take() {
            notify_fanotify_prepared_event(event);
        }
    }
    if let Some(event) = rename_event {
        notify_fanotify_prepared_event(event);
    }
    if let Some(event) = move_from_event {
        notify_fanotify_prepared_event(event);
    } else {
        notify_fanotify_move_from(old_dirfd, &old_path, is_dir);
    }
    notify_fanotify_move_to(new_dirfd, &new_path, is_dir);
    notify_inotify_dirent(old_dirfd, &old_path, IN_MOVED_FROM, is_dir, cookie);
    notify_inotify_dirent(new_dirfd, &new_path, IN_MOVED_TO, is_dir, cookie);
    if let Some(target) = inotify_target {
        notify_inotify_target(target, IN_MOVE_SELF);
    }
    if let Some(event) = move_self_event {
        notify_fanotify_prepared_event(event);
    }
    Ok(0)
}

pub fn sys_sync() -> AxResult<isize> {
    // Flush the root filesystem (ext4 on contest images).
    FS_CONTEXT.lock().root_dir().filesystem().flush()?;
    Ok(0)
}

pub fn sys_syncfs(fd: i32) -> AxResult<isize> {
    if let Ok(file) = File::from_fd(fd) {
        file.inner().location().filesystem().flush()?;
        return Ok(0);
    }
    if let Ok(dir) = Directory::from_fd(fd) {
        dir.inner().filesystem().flush()?;
        return Ok(0);
    }
    Err(AxError::BadFileDescriptor)
}
