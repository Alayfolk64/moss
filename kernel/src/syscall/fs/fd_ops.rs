use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::ToString,
    sync::Arc,
    vec::Vec,
};
use core::ffi::{c_char, c_int};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, FileBackend, OpenOptions, OpenResult};
use axfs_ng_vfs::{
    DirEntry, FileNode, Location, NodePermission, NodeType, Reference, VfsError, path::Path,
};
use axio::{Seek, SeekFrom};
use axtask::{current, yield_now};
use bitflags::bitflags;
use kernel_guard::NoPreempt;
use linux_raw_sys::general::*;
use scope_local::ActiveScope;
use spin::RwLock;
use starry_signal::Signo;

use super::{
    fanotify::{
        check_fanotify_open_permission, notify_fanotify_close, notify_fanotify_create,
        notify_fanotify_fd_close, notify_fanotify_open,
    },
    inotify::{notify_inotify_dirent, notify_inotify_file},
    protected_symlinks_enabled,
};
use crate::{
    file::{
        Directory, FD_TABLE, File, FileLike, PIPE_MAX_SIZE, Pipe, add_file_like,
        add_file_like_from, async_fcntl_get_owner, async_fcntl_get_signal, async_fcntl_set_owner,
        async_fcntl_set_signal, close_file_like, get_file_like, resolve_at, with_fs,
    },
    mm::{UserConstPtr, UserPtr, vm_load_string},
    pseudofs::{Device, dev::tty},
    syscall::{
        sys::{sys_getegid, sys_geteuid},
        task::current_has_cap,
    },
    task::AsThread,
};

/// Convert open flags to [`OpenOptions`].
fn flags_to_options(flags: c_int, mode: __kernel_mode_t, (uid, gid): (u32, u32)) -> OpenOptions {
    let flags = flags as u32;
    let mut options = OpenOptions::new();
    options.mode(mode).user(uid, gid);
    match flags & 0b11 {
        O_RDONLY => options.read(true),
        O_WRONLY => options.write(true),
        _ => options.read(true).write(true),
    };
    if flags & O_APPEND != 0 {
        options.append(true);
    }
    if flags & O_TRUNC != 0 {
        options.truncate(true);
    }
    if flags & O_CREAT != 0 {
        options.create(true);
    }
    if flags & O_PATH != 0 {
        options.path(true);
    }
    if flags & O_EXCL != 0 {
        options.create_new(true);
    }
    if flags & O_DIRECTORY != 0 {
        options.directory(true);
    }
    if flags & O_NOFOLLOW != 0 {
        options.no_follow(true);
    }
    if flags & O_DIRECT != 0 {
        options.direct(true);
    }
    options
}

fn open_access_mode(flags: u32) -> u32 {
    if flags & O_PATH != 0 {
        return 0;
    }

    let mut mode = match flags & O_ACCMODE {
        O_WRONLY => W_OK as u32,
        O_RDWR => (R_OK | W_OK) as u32,
        _ => R_OK as u32,
    };
    if flags & O_TRUNC != 0 {
        mode |= W_OK as u32;
    }
    mode
}

fn has_open_access(mode: u32, st_mode: u32, st_uid: u32, st_gid: u32, uid: u32, gid: u32) -> bool {
    if mode == 0 || uid == 0 {
        return true;
    }

    let shift = if uid == st_uid {
        6
    } else if gid == st_gid {
        3
    } else {
        0
    };

    let mut required = 0;
    if mode & R_OK as u32 != 0 {
        required |= 0o4 << shift;
    }
    if mode & W_OK as u32 != 0 {
        required |= 0o2 << shift;
    }

    st_mode & required == required
}

fn check_open_access(dirfd: c_int, path: &str, flags: u32, (uid, gid): (u32, u32)) -> AxResult {
    let mode = open_access_mode(flags);
    if mode == 0 {
        return Ok(());
    }

    let resolve_flags = if flags & O_NOFOLLOW != 0 {
        AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    let loc = match resolve_at(dirfd, Some(path), resolve_flags) {
        Ok(result) => match result.into_file() {
            Some(loc) => loc,
            None => return Ok(()),
        },
        Err(err) if flags & O_CREAT != 0 && err.canonicalize() == AxError::NotFound => {
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    let metadata = loc.metadata()?;
    if has_open_access(
        mode,
        metadata.mode.bits() as u32,
        metadata.uid,
        metadata.gid,
        uid,
        gid,
    ) {
        Ok(())
    } else {
        Err(AxError::PermissionDenied)
    }
}

fn check_protected_symlink_follow(dirfd: c_int, path: &str, flags: u32, uid: u32) -> AxResult {
    if !protected_symlinks_enabled() || flags & O_NOFOLLOW != 0 {
        return Ok(());
    }

    with_fs(dirfd, |fs| {
        let (parent, name) = fs.resolve_parent(Path::new(path))?;
        let symlink = match parent.lookup_no_follow(name.as_ref()) {
            Ok(symlink) => symlink,
            Err(err) if flags & O_CREAT != 0 && err.canonicalize() == VfsError::NotFound => {
                return Ok(());
            }
            Err(err) => return Err(err.into()),
        };
        if symlink.node_type() != NodeType::Symlink {
            return Ok(());
        }

        let parent_meta = parent.metadata()?;
        let parent_mode = parent_meta.mode.bits() as u32;
        let sticky_world_writable = parent_mode & S_ISVTX != 0 && parent_mode & S_IWOTH != 0;
        if !sticky_world_writable {
            return Ok(());
        }

        let symlink_meta = symlink.metadata()?;
        if uid == symlink_meta.uid || parent_meta.uid == symlink_meta.uid {
            Ok(())
        } else {
            Err(AxError::PermissionDenied)
        }
    })
}

fn check_nosymfollow_mount_open(dirfd: c_int, path: &str, flags: u32) -> AxResult {
    if flags & (O_NOFOLLOW | O_PATH) != 0 {
        return Ok(());
    }

    with_fs(dirfd, |fs| {
        let (parent, name) = match fs.resolve_parent(Path::new(path)) {
            Ok(result) => result,
            Err(err) if err.canonicalize() == VfsError::InvalidInput => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        match parent.lookup_no_follow(name.as_ref()) {
            Ok(loc)
                if loc.node_type() == NodeType::Symlink
                    && loc.mount_flags() & MS_NOSYMFOLLOW != 0 =>
            {
                Err(AxError::FilesystemLoop)
            }
            Ok(_) => Ok(()),
            Err(err) if flags & O_CREAT != 0 && err.canonicalize() == VfsError::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    })
}

fn check_mount_open_constraints(dirfd: c_int, path: &str, flags: u32) -> AxResult {
    check_nosymfollow_mount_open(dirfd, path, flags)?;

    let wants_write = flags & O_PATH == 0
        && ((flags & O_ACCMODE) != O_RDONLY || flags & (O_CREAT | O_TRUNC) != 0);
    let resolve_flags = if flags & O_NOFOLLOW != 0 {
        AT_SYMLINK_NOFOLLOW
    } else {
        0
    };

    match resolve_at(dirfd, Some(path), resolve_flags) {
        Ok(result) => {
            let stat = result.stat()?;
            let Some(loc) = result.into_file() else {
                return Ok(());
            };
            if wants_write
                && flags & (O_CREAT | O_EXCL) != (O_CREAT | O_EXCL)
                && loc.mount_flags() & MS_RDONLY != 0
            {
                return Err(AxError::ReadOnlyFilesystem);
            }
            let node_type = loc.metadata()?.node_type;
            if loc.mount_flags() & MS_NODEV != 0
                && (matches!(node_type, NodeType::CharacterDevice | NodeType::BlockDevice)
                    || matches!(stat.mode & S_IFMT, S_IFCHR | S_IFBLK))
            {
                return Err(AxError::PermissionDenied);
            }
        }
        Err(err) if flags & O_CREAT != 0 && err.canonicalize() == AxError::NotFound => {
            if wants_write {
                with_fs(dirfd, |fs| {
                    let (dir, _) = fs.resolve_parent(Path::new(path))?;
                    if dir.mount_flags() & MS_RDONLY != 0 {
                        return Err(AxError::ReadOnlyFilesystem);
                    }
                    Ok(())
                })?;
            }
        }
        Err(err) => return Err(err),
    }
    Ok(())
}

fn check_open_target_constraints(
    dirfd: c_int,
    path: &str,
    flags: u32,
    (euid, _egid): (u32, u32),
) -> AxResult {
    let resolve_flags = if flags & O_NOFOLLOW != 0 {
        AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    let loc = match resolve_at(dirfd, Some(path), resolve_flags) {
        Ok(result) => match result.into_file() {
            Some(loc) => loc,
            None => return Ok(()),
        },
        Err(err) if flags & O_CREAT != 0 && err.canonicalize() == AxError::NotFound => {
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    let metadata = loc.metadata()?;

    if flags & O_NOFOLLOW != 0
        && flags & O_PATH == 0
        && metadata.node_type == NodeType::Symlink
    {
        return Err(AxError::FilesystemLoop);
    }

    if flags & O_PATH == 0 && metadata.node_type == NodeType::Directory {
        let wants_write = flags & O_ACCMODE != O_RDONLY || flags & O_TRUNC != 0;
        let create_existing = flags & O_CREAT != 0 && flags & O_EXCL == 0;
        if wants_write || create_existing {
            return Err(AxError::IsADirectory);
        }
    }

    if metadata.node_type == NodeType::Fifo
        && flags & O_NONBLOCK != 0
        && flags & O_ACCMODE == O_WRONLY
    {
        return Err(AxError::from(LinuxError::ENXIO));
    }

    if flags & O_NOATIME != 0 && euid != 0 && euid != metadata.uid {
        return Err(AxError::from(LinuxError::EPERM));
    }

    Ok(())
}

fn creation_mode_and_owner(
    dirfd: c_int,
    path: &str,
    flags: u32,
    mode: __kernel_mode_t,
    (uid, gid): (u32, u32),
) -> AxResult<(__kernel_mode_t, (u32, u32))> {
    if flags & O_CREAT == 0 {
        return Ok((mode, (uid, gid)));
    }

    let mut mode = mode;
    mode &= !(S_ISUID as __kernel_mode_t);

    let file_gid = with_fs(dirfd, |fs| {
        let (parent, _) = fs.resolve_parent(Path::new(path))?;
        let metadata = parent.metadata()?;
        Ok(if metadata.mode.contains(NodePermission::SET_GID) {
            metadata.gid
        } else {
            gid
        })
    })?;

    if mode & S_ISGID as __kernel_mode_t != 0 && uid != 0 && file_gid != gid {
        mode &= !(S_ISGID as __kernel_mode_t);
    }

    Ok((mode, (uid, file_gid)))
}

fn open_with_create_follow(
    dirfd: c_int,
    path: &str,
    options: &OpenOptions,
    flags: u32,
) -> AxResult<OpenResult> {
    match with_fs(dirfd, |fs| options.open(fs, path)) {
        Err(err)
            if flags & O_CREAT != 0
                && flags & (O_EXCL | O_NOFOLLOW) == 0
                && err.canonicalize() == AxError::NotFound =>
        {
            with_fs(dirfd, |fs| {
                let (parent, name) = fs.resolve_parent(Path::new(path))?;
                let symlink = parent.lookup_no_follow(name.as_ref())?;
                if symlink.node_type() != NodeType::Symlink {
                    return Err(AxError::NotFound);
                }
                let target = symlink.read_link()?;
                if target.is_empty() {
                    return Err(AxError::NotFound);
                }
                let fs = fs.with_current_dir(parent)?;
                options.open(&fs, target.as_str()).map_err(Into::into)
            })
        }
        result => result,
    }
}

fn add_to_fd(result: OpenResult, flags: u32) -> AxResult<i32> {
    let f: Arc<dyn FileLike> = match result {
        OpenResult::File(mut file) => {
            // /dev/xx handling
            if let Ok(device) = file.location().entry().downcast::<Device>() {
                let inner = device.inner().as_any();
                if let Some(ptmx) = inner.downcast_ref::<tty::Ptmx>() {
                    // Opening /dev/ptmx creates a new pseudo-terminal
                    let (master, pty_number) = ptmx.create_pty()?;
                    // TODO: this is cursed
                    let pts = FS_CONTEXT.lock().resolve("/dev/pts")?;
                    let entry = DirEntry::new_file(
                        FileNode::new(master),
                        NodeType::CharacterDevice,
                        Reference::new(Some(pts.entry().clone()), pty_number.to_string()),
                    );
                    let loc = Location::new(file.location().mountpoint().clone(), entry);
                    file = axfs::File::new(FileBackend::Direct(loc), file.flags());
                } else if inner.is::<tty::CurrentTty>() {
                    let term = current()
                        .as_thread()
                        .proc_data
                        .proc
                        .group()
                        .session()
                        .terminal()
                        .ok_or(AxError::NotFound)?;
                    let path = if term.is::<tty::NTtyDriver>() {
                        "/dev/console".to_string()
                    } else if let Some(pts) = term.downcast_ref::<tty::PtyDriver>() {
                        format!("/dev/pts/{}", pts.pty_number())
                    } else {
                        panic!("unknown terminal type")
                    };
                    let loc = FS_CONTEXT.lock().resolve(&path)?;
                    file = axfs::File::new(FileBackend::Direct(loc), file.flags());
                }
            }
            Arc::new(File::new_with_status_flags(file, flags))
        }
        OpenResult::Dir(dir) => Arc::new(Directory::new(dir)),
    };
    if flags & O_NONBLOCK != 0 {
        f.set_nonblocking(true)?;
    }
    check_fanotify_open_permission(&f)?;
    let opened = f.clone();
    let fd = add_file_like(f, flags & O_CLOEXEC != 0)?;
    notify_inotify_file(opened.as_ref(), IN_OPEN);
    notify_fanotify_open(&opened);
    Ok(fd)
}

/// Open or create a file.
/// fd: file descriptor
/// filename: file path to be opened or created
/// flags: open flags
/// mode: see man 7 inode
/// return new file descriptor if succeed, or return -1.
pub fn sys_openat(
    dirfd: c_int,
    path: *const c_char,
    flags: i32,
    mode: __kernel_mode_t,
) -> AxResult<isize> {
    let path = vm_load_string(path)?;
    if path.is_empty() {
        return Err(AxError::NotFound);
    }
    debug!("sys_openat <= {dirfd} {path:?} {flags:#o} {mode:#o}");

    let flags_u32 = flags as u32;
    if flags_u32 & __O_TMPFILE != 0 {
        return Err(AxError::OperationNotSupported);
    }

    let mode = mode & !current().as_thread().proc_data.umask();
    let ids = (sys_geteuid()? as u32, sys_getegid()? as u32);
    let (mode, ids) = creation_mode_and_owner(dirfd, path.as_str(), flags_u32, mode, ids)?;
    let existed_before_create =
        flags_u32 & O_CREAT != 0 && resolve_at(dirfd, Some(path.as_str()), 0).is_ok();
    check_mount_open_constraints(dirfd, path.as_str(), flags_u32)?;
    check_open_target_constraints(dirfd, path.as_str(), flags_u32, ids)?;

    // axfs::OpenOptions rejects (read-only + create), but Linux permits
    // O_RDONLY|O_CREAT. Handle it locally without modifying third-party crates.
    if flags_u32 & O_CREAT != 0 && flags_u32 & O_PATH == 0 && (flags_u32 & O_ACCMODE) == O_RDONLY {
        let open_flags = (flags_u32 & !(O_CREAT | O_EXCL)) as i32;
        let open_options = flags_to_options(open_flags, mode, ids);

        if flags_u32 & O_EXCL == 0 {
            check_protected_symlink_follow(dirfd, path.as_str(), flags_u32, ids.0)?;
            check_open_access(dirfd, path.as_str(), flags_u32, ids)?;
            match open_with_create_follow(dirfd, path.as_str(), &open_options, flags_u32) {
                Ok(result) => {
                    if flags_u32 & O_CREAT != 0 && !existed_before_create {
                        notify_inotify_dirent(dirfd, &path, IN_CREATE, false, 0);
                    }
                    let fd = add_to_fd(result, flags_u32)?;
                    if flags_u32 & O_CREAT != 0 && !existed_before_create {
                        notify_fanotify_create(dirfd, &path, false);
                    }
                    return Ok(fd as isize);
                }
                Err(err) if err.canonicalize() == AxError::NotFound => {}
                Err(err) => return Err(err),
            }
        }

        let create_flags = ((flags_u32 & !O_ACCMODE) | O_WRONLY) as i32;
        let create_options = flags_to_options(create_flags, mode, ids);
        open_with_create_follow(dirfd, path.as_str(), &create_options, flags_u32)?;

        if !existed_before_create {
            notify_inotify_dirent(dirfd, &path, IN_CREATE, false, 0);
        }
        let fd = open_with_create_follow(dirfd, path.as_str(), &open_options, flags_u32)
            .and_then(|it| add_to_fd(it, flags_u32))?;
        if !existed_before_create {
            notify_fanotify_create(dirfd, &path, false);
        }
        return Ok(fd as isize);
    }

    let options = flags_to_options(flags, mode, ids);
    check_protected_symlink_follow(dirfd, path.as_str(), flags_u32, ids.0)?;
    check_open_access(dirfd, path.as_str(), flags_u32, ids)?;
    if flags_u32 & O_CREAT != 0 && !existed_before_create {
        let result = open_with_create_follow(dirfd, path.as_str(), &options, flags_u32)?;
        notify_inotify_dirent(dirfd, &path, IN_CREATE, false, 0);
        let fd = add_to_fd(result, flags as _)?;
        notify_fanotify_create(dirfd, &path, false);
        return Ok(fd as isize);
    }
    let fd = open_with_create_follow(dirfd, path.as_str(), &options, flags_u32)
        .and_then(|it| add_to_fd(it, flags as _))?;
    Ok(fd as isize)
}

/// Open a file by `filename` and insert it into the file descriptor table.
///
/// Return its index in the file table (`fd`). Return `EMFILE` if it already
/// has the maximum number of files open.
#[cfg(target_arch = "x86_64")]
pub fn sys_open(path: *const c_char, flags: i32, mode: __kernel_mode_t) -> AxResult<isize> {
    sys_openat(AT_FDCWD as _, path, flags, mode)
}

pub fn sys_close(fd: c_int) -> AxResult<isize> {
    debug!("sys_close <= {fd}");
    let file_like = get_file_like(fd)?;
    let writable = file_like.status_flags() & O_ACCMODE != O_RDONLY;
    release_posix_locks_for_file_like(&file_like);
    release_file_lease_for_file_like(&file_like);
    notify_fanotify_fd_close(&file_like);
    close_file_like(fd)?;
    notify_inotify_file(
        file_like.as_ref(),
        if writable {
            IN_CLOSE_WRITE
        } else {
            IN_CLOSE_NOWRITE
        },
    );
    notify_fanotify_close(&file_like, writable);
    release_bsd_flocks_for_file_like_if_last(&file_like);
    Ok(0)
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    struct CloseRangeFlags: u32 {
        const UNSHARE = 1 << 1;
        const CLOEXEC = 1 << 2;
    }
}

pub fn sys_close_range(first: u32, last: u32, flags: u32) -> AxResult<isize> {
    if last < first {
        return Err(AxError::InvalidInput);
    }
    let flags = CloseRangeFlags::from_bits(flags).ok_or(AxError::InvalidInput)?;
    debug!("sys_close_range <= fds: [{first}, {last}], flags: {flags:?}");
    let cloexec = flags.contains(CloseRangeFlags::CLOEXEC);
    let first = first as usize;
    let last = last as usize;

    if flags.contains(CloseRangeFlags::UNSHARE) {
        // 1) Snapshot current FD table.
        let cloned = FD_TABLE.read().clone();

        // 2) Detach current task's scope-local FD table pointer.
        // The running task keeps a leaked scope read-lock (see TaskExt::on_enter/on_leave),
        // so we must temporarily drop it before taking scope.write(). Keep preemption
        // disabled during this window to avoid task switch with a mismatched lock count.
        let curr = current();
        let thr = curr.as_thread();
        let _preempt_guard = NoPreempt::new();
        ActiveScope::set_global();
        unsafe { thr.proc_data.scope.force_read_decrement() };
        let mut scope = thr.proc_data.scope.write();
        *FD_TABLE.scope_mut(&mut scope) = Arc::new(spin::RwLock::new(cloned));
        drop(scope);
        let scope = thr.proc_data.scope.read();
        unsafe { ActiveScope::set(&scope) };
        core::mem::forget(scope);
    }

    let mut fd_table = FD_TABLE.write();
    if let Some(max_index) = fd_table.ids().next_back() {
        for fd in first..=last.min(max_index) {
            if cloexec {
                if let Some(f) = fd_table.get_mut(fd) {
                    f.cloexec = true;
                }
            } else {
                if let Some(file_desc) = fd_table.remove(fd) {
                    release_posix_locks_for_file_like(&file_desc.inner);
                    release_file_lease_for_file_like(&file_desc.inner);
                    release_bsd_flocks_for_file_like_if_last(&file_desc.inner);
                }
            }
        }
    }

    Ok(0)
}

fn dup_fd(old_fd: c_int, min_fd: usize, cloexec: bool) -> AxResult<isize> {
    let f = get_file_like(old_fd)?;
    let new_fd = add_file_like_from(f, cloexec, min_fd)?;
    Ok(new_fd as _)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PosixLockKey {
    dev: u64,
    ino: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LockRange {
    /// Inclusive start byte.
    start: u64,
    /// Exclusive end byte. `None` means to EOF/infinity.
    end: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PosixRecordLock {
    owner: u32,
    lock_type: i16,
    range: LockRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileLease {
    owner: u32,
    lease_type: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BsdFileLock {
    owner: usize,
    exclusive: bool,
}

static POSIX_LOCKS: RwLock<BTreeMap<PosixLockKey, Vec<PosixRecordLock>>> =
    RwLock::new(BTreeMap::new());
static POSIX_WAIT_GRAPH: RwLock<BTreeMap<u32, Vec<u32>>> = RwLock::new(BTreeMap::new());
static FILE_LEASES: RwLock<BTreeMap<PosixLockKey, FileLease>> = RwLock::new(BTreeMap::new());
static BSD_FLOCKS: RwLock<BTreeMap<PosixLockKey, Vec<BsdFileLock>>> = RwLock::new(BTreeMap::new());

fn has_unblocked_pending_signal() -> bool {
    let curr = current();
    let signal = &curr.as_thread().signal;
    let pending = signal.pending();
    let blocked = signal.blocked();
    !(pending & !blocked).is_empty()
}

fn posix_lock_key(file: &File) -> AxResult<PosixLockKey> {
    let meta = file.inner().location().metadata()?;
    Ok(PosixLockKey {
        dev: meta.device,
        ino: meta.inode,
    })
}

fn file_description_owner(file_like: &Arc<dyn FileLike>) -> usize {
    let ptr: *const dyn FileLike = Arc::as_ptr(file_like);
    ptr as *const () as usize
}

fn flock_mode(operation: c_int) -> AxResult<(bool, bool)> {
    let shared = operation & LOCK_SH as c_int != 0;
    let exclusive = operation & LOCK_EX as c_int != 0;
    let unlock = operation & LOCK_UN as c_int != 0;
    let nonblock = operation & LOCK_NB as c_int != 0;
    let known = LOCK_SH | LOCK_EX | LOCK_UN | LOCK_NB;

    if operation & !(known as c_int) != 0 || (shared as u8 + exclusive as u8 + unlock as u8) != 1 {
        return Err(AxError::InvalidInput);
    }

    Ok((exclusive, nonblock))
}

fn bsd_flock_conflicts(locks: &[BsdFileLock], owner: usize, exclusive: bool) -> bool {
    locks
        .iter()
        .any(|lock| lock.owner != owner && (exclusive || lock.exclusive))
}

fn validate_lock_type(lock_type: i16) -> AxResult<()> {
    if lock_type == F_RDLCK as i16 || lock_type == F_WRLCK as i16 || lock_type == F_UNLCK as i16 {
        Ok(())
    } else {
        Err(AxError::InvalidInput)
    }
}

fn validate_lock_whence(lock_whence: i16) -> AxResult<()> {
    match i32::from(lock_whence) {
        v if v == SEEK_SET as i32 || v == SEEK_CUR as i32 || v == SEEK_END as i32 => Ok(()),
        _ => Err(AxError::InvalidInput),
    }
}

fn validate_lease_type(lease_type: i16) -> AxResult<()> {
    if lease_type == F_RDLCK as i16 || lease_type == F_WRLCK as i16 || lease_type == F_UNLCK as i16
    {
        Ok(())
    } else {
        Err(AxError::InvalidInput)
    }
}

fn validate_owner_type(owner_type: i32) -> AxResult<()> {
    match owner_type as u32 {
        F_OWNER_TID | F_OWNER_PID | F_OWNER_PGRP => Ok(()),
        _ => Err(AxError::InvalidInput),
    }
}

fn validate_lock_access(file: &File, lock_type: i16) -> AxResult<()> {
    let mode = file.status_flags() & O_ACCMODE;
    if (lock_type == F_RDLCK as i16 && mode == O_WRONLY)
        || (lock_type == F_WRLCK as i16 && mode == O_RDONLY)
    {
        return Err(AxError::BadFileDescriptor);
    }
    Ok(())
}

fn validate_lease_access(file: &File, lease_type: i16) -> AxResult<()> {
    let mode = file.status_flags() & O_ACCMODE;
    if (lease_type == F_RDLCK as i16 && mode != O_RDONLY)
        || (lease_type == F_WRLCK as i16 && mode == O_RDONLY)
    {
        return Err(AxError::from(LinuxError::EAGAIN));
    }
    Ok(())
}

fn normalize_range(file: &File, l_whence: i16, l_start: i64, l_len: i64) -> AxResult<LockRange> {
    let whence = i32::from(l_whence);
    let base = match whence {
        v if v == SEEK_SET as i32 => 0_i128,
        v if v == SEEK_CUR as i32 => i128::from(file.inner().seek(SeekFrom::Current(0))?),
        v if v == SEEK_END as i32 => i128::from(file.inner().len()?),
        _ => return Err(AxError::InvalidInput),
    };

    let mut start = base
        .checked_add(i128::from(l_start))
        .ok_or(AxError::InvalidInput)?;
    let mut end = None;
    if l_len > 0 {
        end = Some(
            start
                .checked_add(i128::from(l_len))
                .ok_or(AxError::InvalidInput)?,
        );
    } else if l_len < 0 {
        let old_start = start;
        start = start
            .checked_add(i128::from(l_len))
            .ok_or(AxError::InvalidInput)?;
        end = Some(old_start);
    }

    if start < 0 {
        return Err(AxError::InvalidInput);
    }
    if let Some(end) = end
        && end <= start
    {
        return Err(AxError::InvalidInput);
    }

    let start = u64::try_from(start).map_err(|_| AxError::InvalidInput)?;
    let end = end
        .map(|it| u64::try_from(it).map_err(|_| AxError::InvalidInput))
        .transpose()?;
    Ok(LockRange { start, end })
}

fn overlaps(a: LockRange, b: LockRange) -> bool {
    let a_ends_after_b_starts = a.end.is_none_or(|end| end > b.start);
    let b_ends_after_a_starts = b.end.is_none_or(|end| end > a.start);
    a_ends_after_b_starts && b_ends_after_a_starts
}

fn ends_after(end: Option<u64>, point: u64) -> bool {
    end.is_none_or(|it| it > point)
}

fn max_end(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, _) | (_, None) => None,
        (Some(a), Some(b)) => Some(a.max(b)),
    }
}

fn lock_conflicts(req_type: i16, existing_type: i16) -> bool {
    if req_type == F_WRLCK as i16 {
        existing_type == F_WRLCK as i16 || existing_type == F_RDLCK as i16
    } else {
        req_type == F_RDLCK as i16 && existing_type == F_WRLCK as i16
    }
}

fn lock_conflict_error(req_type: i16) -> AxError {
    if req_type == F_RDLCK as i16 {
        AxError::from(LinuxError::EACCES)
    } else {
        AxError::from(LinuxError::EAGAIN)
    }
}

fn find_conflict(
    locks: &[PosixRecordLock],
    owner: u32,
    req_type: i16,
    req_range: LockRange,
) -> Option<PosixRecordLock> {
    locks.iter().copied().find(|lock| {
        lock.owner != owner
            && overlaps(lock.range, req_range)
            && lock_conflicts(req_type, lock.lock_type)
    })
}

fn collect_conflict_owners(
    locks: &[PosixRecordLock],
    owner: u32,
    req_type: i16,
    req_range: LockRange,
) -> Vec<u32> {
    let mut owners = Vec::new();
    for lock in locks {
        if lock.owner == owner
            || !overlaps(lock.range, req_range)
            || !lock_conflicts(req_type, lock.lock_type)
        {
            continue;
        }
        if !owners.contains(&lock.owner) {
            owners.push(lock.owner);
        }
    }
    owners
}

fn clear_wait_dependencies(waiter: u32) {
    POSIX_WAIT_GRAPH.write().remove(&waiter);
}

fn set_wait_dependencies(waiter: u32, blockers: &[u32]) {
    let mut uniq = Vec::new();
    for blocker in blockers {
        if *blocker != waiter && !uniq.contains(blocker) {
            uniq.push(*blocker);
        }
    }
    POSIX_WAIT_GRAPH.write().insert(waiter, uniq);
}

fn has_wait_path(start: u32, target: u32, graph: &BTreeMap<u32, Vec<u32>>) -> bool {
    let mut stack = Vec::new();
    stack.push(start);
    let mut visited = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if !visited.insert(node) {
            continue;
        }
        if let Some(nexts) = graph.get(&node) {
            stack.extend(nexts.iter().copied());
        }
    }
    false
}

fn would_deadlock(waiter: u32, blockers: &[u32]) -> bool {
    let graph = POSIX_WAIT_GRAPH.read();
    blockers
        .iter()
        .any(|blocker| *blocker == waiter || has_wait_path(*blocker, waiter, &graph))
}

fn push_if_non_empty(
    out: &mut Vec<PosixRecordLock>,
    owner: u32,
    lock_type: i16,
    start: u64,
    end: Option<u64>,
) {
    if end.is_some_and(|it| it <= start) {
        return;
    }
    out.push(PosixRecordLock {
        owner,
        lock_type,
        range: LockRange { start, end },
    });
}

fn apply_owner_lock(locks: &mut Vec<PosixRecordLock>, owner: u32, req_type: i16, req: LockRange) {
    let mut next = Vec::with_capacity(locks.len() + 1);

    for lock in locks.drain(..) {
        if lock.owner != owner || !overlaps(lock.range, req) {
            next.push(lock);
            continue;
        }

        // Keep the left part that does not overlap with the requested range.
        if lock.range.start < req.start {
            push_if_non_empty(
                &mut next,
                owner,
                lock.lock_type,
                lock.range.start,
                Some(req.start),
            );
        }

        // Keep the right part that does not overlap with the requested range.
        if let Some(req_end) = req.end
            && ends_after(lock.range.end, req_end)
        {
            push_if_non_empty(&mut next, owner, lock.lock_type, req_end, lock.range.end);
        }
    }

    if req_type != F_UNLCK as i16 {
        next.push(PosixRecordLock {
            owner,
            lock_type: req_type,
            range: req,
        });
    }

    next.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then_with(|| {
                a.range
                    .end
                    .unwrap_or(u64::MAX)
                    .cmp(&b.range.end.unwrap_or(u64::MAX))
            })
            .then_with(|| a.owner.cmp(&b.owner))
            .then_with(|| a.lock_type.cmp(&b.lock_type))
    });

    let mut merged: Vec<PosixRecordLock> = Vec::with_capacity(next.len());
    for lock in next {
        if let Some(last) = merged.last_mut()
            && last.owner == lock.owner
            && last.lock_type == lock.lock_type
            && last.range.end.is_none_or(|end| lock.range.start <= end)
        {
            last.range.end = max_end(last.range.end, lock.range.end);
            continue;
        }
        merged.push(lock);
    }
    *locks = merged;
}

fn set_flock_from_lock(dst: &mut flock64, src: PosixRecordLock) {
    dst.l_type = src.lock_type;
    dst.l_whence = SEEK_SET as _;
    dst.l_start = src.range.start as _;
    dst.l_len = src
        .range
        .end
        .map_or(0, |end| end.saturating_sub(src.range.start) as _);
    dst.l_pid = src.owner as _;
}

fn release_posix_locks_for_file_like(file_like: &Arc<dyn FileLike>) {
    let Some(file) = file_like.downcast_ref::<File>() else {
        return;
    };
    let Ok(key) = posix_lock_key(file) else {
        return;
    };
    let owner = current().as_thread().proc_data.proc.pid();
    let mut map = POSIX_LOCKS.write();
    if let Some(locks) = map.get_mut(&key) {
        locks.retain(|lock| lock.owner != owner);
        if locks.is_empty() {
            map.remove(&key);
        }
    }
}

fn release_file_lease_for_file_like(file_like: &Arc<dyn FileLike>) {
    let Some(file) = file_like.downcast_ref::<File>() else {
        return;
    };
    let Ok(key) = posix_lock_key(file) else {
        return;
    };
    let owner = current().as_thread().proc_data.proc.pid();
    let mut leases = FILE_LEASES.write();
    if leases.get(&key).is_some_and(|lease| lease.owner == owner) {
        leases.remove(&key);
    }
}

fn release_bsd_flocks_for_file_like(file_like: &Arc<dyn FileLike>) {
    let Some(file) = file_like.downcast_ref::<File>() else {
        return;
    };
    let Ok(key) = posix_lock_key(file) else {
        return;
    };
    let owner = file_description_owner(file_like);
    let mut map = BSD_FLOCKS.write();
    if let Some(locks) = map.get_mut(&key) {
        locks.retain(|lock| lock.owner != owner);
        if locks.is_empty() {
            map.remove(&key);
        }
    }
}

fn release_bsd_flocks_for_file_like_if_last(file_like: &Arc<dyn FileLike>) {
    if Arc::strong_count(file_like) == 1 {
        release_bsd_flocks_for_file_like(file_like);
    }
}

pub fn close_all_files_for_current_process() {
    let fd_table_ref = FD_TABLE.clone();
    // CLONE_FILES shares the table Arc across process scopes; only the last
    // process that references it should close the underlying descriptors.
    if Arc::strong_count(&fd_table_ref) > 2 {
        return;
    }
    drop(fd_table_ref);

    let file_likes = {
        let fd_table = FD_TABLE.read();
        fd_table
            .ids()
            .filter_map(|fd| fd_table.get(fd).map(|file_desc| file_desc.inner.clone()))
            .collect::<Vec<_>>()
    };

    for file_like in &file_likes {
        release_posix_locks_for_file_like(file_like);
        release_file_lease_for_file_like(file_like);
        release_bsd_flocks_for_file_like(file_like);
        notify_fanotify_fd_close(file_like);
    }

    let mut fd_table = FD_TABLE.write();
    let fds = fd_table.ids().collect::<Vec<_>>();
    for fd in fds {
        fd_table.remove(fd);
    }
}

pub fn release_posix_locks_for_pid(pid: u32) {
    let mut map = POSIX_LOCKS.write();
    map.retain(|_, locks| {
        locks.retain(|lock| lock.owner != pid);
        !locks.is_empty()
    });
    clear_wait_dependencies(pid);
}

pub fn release_file_leases_for_pid(pid: u32) {
    // Best-effort cleanup on process exit: never block exit path on a
    // contended global lease map, otherwise parent waiters may stall.
    if let Some(mut leases) = FILE_LEASES.try_write() {
        leases.retain(|_, lease| lease.owner != pid);
    }
}

fn handle_getlk(fd: c_int, arg: usize) -> AxResult<isize> {
    let file = File::from_fd(fd)?;
    let lock = UserPtr::<flock64>::from(arg).get_as_mut()?;
    let req_type = lock.l_type;
    validate_lock_type(req_type)?;
    if req_type == F_UNLCK as i16 {
        lock.l_type = F_UNLCK as _;
        return Ok(0);
    }
    validate_lock_whence(lock.l_whence)?;

    validate_lock_access(&file, req_type)?;
    let key = posix_lock_key(&file)?;
    let req_range = normalize_range(&file, lock.l_whence, lock.l_start, lock.l_len)?;
    let owner = current().as_thread().proc_data.proc.pid();

    let map = POSIX_LOCKS.read();
    if let Some(locks) = map.get(&key)
        && let Some(conflict) = find_conflict(locks, owner, req_type, req_range)
    {
        set_flock_from_lock(lock, conflict);
    } else {
        // Keep user-provided fields unchanged except l_type as required by F_GETLK.
        lock.l_type = F_UNLCK as _;
    }
    Ok(0)
}

fn handle_setlk(fd: c_int, arg: usize, blocking: bool) -> AxResult<isize> {
    let lock = UserConstPtr::<flock64>::from(arg).get_as_ref()?;
    let req_type = lock.l_type;
    validate_lock_type(req_type)?;
    validate_lock_whence(lock.l_whence)?;

    let file = File::from_fd(fd)?;
    if req_type != F_UNLCK as i16 {
        validate_lock_access(&file, req_type)?;
    }
    let key = posix_lock_key(&file)?;
    let req_range = normalize_range(&file, lock.l_whence, lock.l_start, lock.l_len)?;
    let owner = current().as_thread().proc_data.proc.pid();

    loop {
        let mut map = POSIX_LOCKS.write();
        let mut should_remove_key = false;
        let mut conflict_owners = Vec::new();
        {
            let locks = map.entry(key).or_default();
            if req_type != F_UNLCK as i16 {
                conflict_owners = collect_conflict_owners(locks, owner, req_type, req_range);
            }
            if conflict_owners.is_empty() {
                apply_owner_lock(locks, owner, req_type, req_range);
                should_remove_key = locks.is_empty();
            }
        }

        if conflict_owners.is_empty() {
            if should_remove_key {
                map.remove(&key);
            }
            clear_wait_dependencies(owner);
            return Ok(0);
        }

        if !blocking {
            clear_wait_dependencies(owner);
            return Err(lock_conflict_error(req_type));
        }
        drop(map);

        if would_deadlock(owner, &conflict_owners) {
            clear_wait_dependencies(owner);
            return Err(AxError::from(LinuxError::EDEADLK));
        }
        set_wait_dependencies(owner, &conflict_owners);

        if has_unblocked_pending_signal() {
            clear_wait_dependencies(owner);
            return Err(AxError::Interrupted);
        }
        yield_now();
    }
}

fn handle_setlease(fd: c_int, arg: usize) -> AxResult<isize> {
    let lease_type = arg as i16;
    validate_lease_type(lease_type)?;

    let file = File::from_fd(fd)?;
    let key = posix_lock_key(&file)?;
    let owner = current().as_thread().proc_data.proc.pid();

    if lease_type == F_UNLCK as i16 {
        let mut leases = FILE_LEASES.write();
        if leases.get(&key).is_some_and(|lease| lease.owner == owner) {
            leases.remove(&key);
        }
        return Ok(0);
    }

    validate_lease_access(&file, lease_type)?;

    let mut leases = FILE_LEASES.write();
    if let Some(lease) = leases.get(&key)
        && lease.owner != owner
    {
        return Err(AxError::from(LinuxError::EAGAIN));
    }

    leases.insert(key, FileLease { owner, lease_type });
    Ok(0)
}

fn handle_getlease(fd: c_int) -> AxResult<isize> {
    let file = File::from_fd(fd)?;
    let key = posix_lock_key(&file)?;
    let owner = current().as_thread().proc_data.proc.pid();

    let lease_type = FILE_LEASES
        .read()
        .get(&key)
        .and_then(|lease| (lease.owner == owner).then_some(lease.lease_type))
        .unwrap_or(F_UNLCK as i16);
    Ok(lease_type as isize)
}

fn handle_setown(fd: c_int, arg: usize) -> AxResult<isize> {
    let file_like = get_file_like(fd)?;
    let owner = arg as isize as i32;
    let (owner_type, owner_pid) = if owner < 0 {
        (F_OWNER_PGRP as i32, owner.saturating_abs())
    } else {
        (F_OWNER_PID as i32, owner)
    };
    async_fcntl_set_owner(&file_like, owner_type, owner_pid);
    Ok(0)
}

fn handle_getown(fd: c_int) -> AxResult<isize> {
    let file_like = get_file_like(fd)?;
    let owner = async_fcntl_get_owner(&file_like);
    let pid = isize::try_from(owner.owner_pid).map_err(|_| AxError::InvalidInput)?;
    if owner.owner_type as u32 == F_OWNER_PGRP {
        Ok(-pid)
    } else {
        Ok(pid)
    }
}

fn handle_setown_ex(fd: c_int, arg: usize) -> AxResult<isize> {
    let file_like = get_file_like(fd)?;
    let owner_ex = UserConstPtr::<f_owner_ex>::from(arg).get_as_ref()?;
    validate_owner_type(owner_ex.type_)?;
    if owner_ex.pid < 0 {
        return Err(AxError::InvalidInput);
    }
    async_fcntl_set_owner(&file_like, owner_ex.type_, owner_ex.pid);
    Ok(0)
}

fn handle_getown_ex(fd: c_int, arg: usize) -> AxResult<isize> {
    let file_like = get_file_like(fd)?;
    let owner = async_fcntl_get_owner(&file_like);
    let owner_ex = UserPtr::<f_owner_ex>::from(arg).get_as_mut()?;
    owner_ex.type_ = owner.owner_type;
    owner_ex.pid = owner.owner_pid;
    Ok(0)
}

fn handle_setsig(fd: c_int, arg: usize) -> AxResult<isize> {
    let file_like = get_file_like(fd)?;
    let signal = arg as i32;
    if signal < 0
        || signal > u8::MAX as i32
        || (signal != 0 && Signo::from_repr(signal as u8).is_none())
    {
        return Err(AxError::InvalidInput);
    }
    async_fcntl_set_signal(&file_like, signal);
    Ok(0)
}

fn handle_getsig(fd: c_int) -> AxResult<isize> {
    let file_like = get_file_like(fd)?;
    Ok(async_fcntl_get_signal(&file_like) as isize)
}

pub fn sys_dup(old_fd: c_int) -> AxResult<isize> {
    debug!("sys_dup <= {old_fd}");
    dup_fd(old_fd, 0, false)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_dup2(old_fd: c_int, new_fd: c_int) -> AxResult<isize> {
    if old_fd == new_fd {
        get_file_like(new_fd)?;
        return Ok(new_fd as _);
    }
    sys_dup3(old_fd, new_fd, 0)
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Dup3Flags: c_int {
        const O_CLOEXEC = O_CLOEXEC as _; // Close on exec
    }
}

pub fn sys_dup3(old_fd: c_int, new_fd: c_int, flags: c_int) -> AxResult<isize> {
    let flags = Dup3Flags::from_bits(flags).ok_or(AxError::InvalidInput)?;
    debug!("sys_dup3 <= old_fd: {old_fd}, new_fd: {new_fd}, flags: {flags:?}");

    if old_fd == new_fd {
        return Err(AxError::InvalidInput);
    }

    let mut fd_table = FD_TABLE.write();
    let mut f = fd_table
        .get(old_fd as _)
        .cloned()
        .ok_or(AxError::BadFileDescriptor)?;
    f.cloexec = flags.contains(Dup3Flags::O_CLOEXEC);

    if let Some(file_desc) = fd_table.remove(new_fd as _) {
        release_posix_locks_for_file_like(&file_desc.inner);
        release_file_lease_for_file_like(&file_desc.inner);
        release_bsd_flocks_for_file_like_if_last(&file_desc.inner);
    }
    fd_table
        .add_at(new_fd as _, f)
        .map_err(|_| AxError::BadFileDescriptor)?;

    Ok(new_fd as _)
}

pub fn sys_fcntl(fd: c_int, cmd: c_int, arg: usize) -> AxResult<isize> {
    debug!("sys_fcntl <= fd: {fd} cmd: {cmd} arg: {arg}");

    match cmd as u32 {
        F_DUPFD => dup_fd(fd, arg, false),
        F_DUPFD_CLOEXEC => dup_fd(fd, arg, true),
        F_SETLK => handle_setlk(fd, arg, false),
        F_SETLKW => handle_setlk(fd, arg, true),
        F_SETOWN => handle_setown(fd, arg),
        F_GETOWN => handle_getown(fd),
        F_SETOWN_EX => handle_setown_ex(fd, arg),
        F_GETOWN_EX => handle_getown_ex(fd, arg),
        F_SETSIG => handle_setsig(fd, arg),
        F_GETSIG => handle_getsig(fd),
        F_SETLEASE => handle_setlease(fd, arg),
        F_OFD_SETLK | F_OFD_SETLKW => Ok(0),
        F_GETLK => handle_getlk(fd, arg),
        F_GETLEASE => handle_getlease(fd),
        F_OFD_GETLK => {
            let arg = UserPtr::<flock64>::from(arg);
            arg.get_as_mut()?.l_type = F_UNLCK as _;
            Ok(0)
        }
        F_SETFL => {
            get_file_like(fd)?.set_status_flags(arg as _)?;
            Ok(0)
        }
        F_GETFL => Ok(get_file_like(fd)?.status_flags() as _),
        F_GETFD => {
            let cloexec = FD_TABLE
                .read()
                .get(fd as _)
                .ok_or(AxError::BadFileDescriptor)?
                .cloexec;
            Ok(if cloexec { FD_CLOEXEC as _ } else { 0 })
        }
        F_SETFD => {
            let cloexec = arg & FD_CLOEXEC as usize != 0;
            FD_TABLE
                .write()
                .get_mut(fd as _)
                .ok_or(AxError::BadFileDescriptor)?
                .cloexec = cloexec;
            Ok(0)
        }
        F_GETPIPE_SZ => {
            let pipe = Pipe::from_fd(fd)?;
            Ok(pipe.capacity() as _)
        }
        F_SETPIPE_SZ => {
            let pipe = Pipe::from_fd(fd)?;
            if arg > (1usize << 31) {
                return Err(AxError::InvalidInput);
            }
            if arg > PIPE_MAX_SIZE && !current_has_cap(CAP_SYS_RESOURCE) {
                return Err(AxError::from(LinuxError::EPERM));
            }
            Ok(pipe.resize(arg)? as _)
        }
        _ => {
            warn!("unsupported fcntl parameters: cmd: {cmd}");
            Err(AxError::InvalidInput)
        }
    }
}

pub fn sys_flock(fd: c_int, operation: c_int) -> AxResult<isize> {
    debug!("flock <= fd: {fd}, operation: {operation}");
    let (exclusive, nonblock) = flock_mode(operation)?;
    let file_like = get_file_like(fd)?;
    let file = file_like
        .downcast_ref::<File>()
        .ok_or(AxError::InvalidInput)?;
    let key = posix_lock_key(file)?;
    let owner = file_description_owner(&file_like);

    if operation & LOCK_UN as c_int != 0 {
        release_bsd_flocks_for_file_like(&file_like);
        return Ok(0);
    }

    loop {
        let mut map = BSD_FLOCKS.write();
        let locks = map.entry(key).or_default();
        if !bsd_flock_conflicts(locks, owner, exclusive) {
            locks.retain(|lock| lock.owner != owner);
            locks.push(BsdFileLock { owner, exclusive });
            return Ok(0);
        }
        if nonblock {
            return Err(AxError::WouldBlock);
        }
        drop(map);
        yield_now();
    }
}
