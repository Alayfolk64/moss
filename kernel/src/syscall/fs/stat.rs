use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
};
use core::{
    ffi::{c_char, c_int},
    mem::MaybeUninit,
};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, OpenOptions, OpenResult};
use axfs_ng_vfs::Location;
use axtask::current;
use linux_raw_sys::general::{
    __kernel_fsid_t, AT_EACCESS, AT_EMPTY_PATH, AT_FDCWD, AT_NO_AUTOMOUNT, AT_STATX_DONT_SYNC,
    AT_STATX_FORCE_SYNC, AT_STATX_SYNC_TYPE, AT_SYMLINK_FOLLOW, AT_SYMLINK_NOFOLLOW, MS_NOATIME,
    MS_NODEV, MS_NODIRATIME, MS_NOEXEC, MS_NOSUID, MS_NOSYMFOLLOW, MS_RDONLY, MS_RELATIME,
    O_ACCMODE, O_CLOEXEC, O_DIRECTORY, O_NONBLOCK, O_PATH, O_RDWR, O_WRONLY, R_OK, S_IFLNK, S_IFMT,
    STATX__RESERVED, W_OK, X_OK, stat, statfs, statx,
};
use spin::RwLock;
use starry_vm::{VmMutPtr, VmPtr, vm_write_slice};

use crate::{
    file::{Directory, File, FileLike, Pipe, add_file_like, get_file_like, resolve_at},
    mm::vm_load_string,
    syscall::{fs::mount::note_mount_access, task::current_has_cap},
    task::AsThread,
};

/// Get the file metadata by `path` and write into `statbuf`.
///
/// Return 0 if success.
#[cfg(target_arch = "x86_64")]
pub fn sys_stat(path: *const c_char, statbuf: *mut stat) -> AxResult<isize> {
    use linux_raw_sys::general::AT_FDCWD;

    sys_fstatat(AT_FDCWD, path, statbuf, 0)
}

/// Get file metadata by `fd` and write into `statbuf`.
///
/// Return 0 if success.
pub fn sys_fstat(fd: i32, statbuf: *mut stat) -> AxResult<isize> {
    sys_fstatat(fd, core::ptr::null(), statbuf, AT_EMPTY_PATH)
}

/// Get the metadata of the symbolic link and write into `buf`.
///
/// Return 0 if success.
#[cfg(target_arch = "x86_64")]
pub fn sys_lstat(path: *const c_char, statbuf: *mut stat) -> AxResult<isize> {
    use linux_raw_sys::general::{AT_FDCWD, AT_SYMLINK_NOFOLLOW};

    sys_fstatat(AT_FDCWD, path, statbuf, AT_SYMLINK_NOFOLLOW)
}

pub fn sys_fstatat(
    dirfd: i32,
    path: *const c_char,
    statbuf: *mut stat,
    flags: u32,
) -> AxResult<isize> {
    const VALID_FLAGS: u32 = AT_EMPTY_PATH | AT_NO_AUTOMOUNT | AT_SYMLINK_NOFOLLOW;

    if flags & !VALID_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }

    let path = if path.is_null() {
        if flags & AT_EMPTY_PATH == 0 {
            return Err(AxError::BadAddress);
        }
        None
    } else {
        Some(vm_load_string(path)?)
    };

    debug!("sys_fstatat <= dirfd: {dirfd}, path: {path:?}, flags: {flags}");

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    check_parent_search_access(
        dirfd,
        path.as_deref(),
        flags,
        proc_data.euid(),
        proc_data.egid(),
    )?;

    let loc = resolve_at(dirfd, path.as_deref(), flags)?;
    statbuf.vm_write(loc.stat()?.into())?;

    Ok(0)
}

const MAX_HANDLE_SZ: u32 = 128;
const STARRY_FILE_HANDLE_BYTES: u32 = 16;
const STARRY_FILE_HANDLE_TYPE: i32 = 1;
const CAP_DAC_READ_SEARCH: u32 = 2;

#[derive(Clone)]
struct HandleRecord {
    path: String,
    mount_id: u64,
}

static HANDLE_RECORDS: RwLock<BTreeMap<[u8; STARRY_FILE_HANDLE_BYTES as usize], HandleRecord>> =
    RwLock::new(BTreeMap::new());

const ST_NOSYMFOLLOW: u32 = 0x2000;

fn mount_flags_to_statfs_flags(flags: u32) -> u32 {
    let mut statfs_flags = flags
        & (MS_RDONLY
            | MS_NODEV
            | MS_NOEXEC
            | MS_NOSUID
            | MS_NOATIME
            | MS_NODIRATIME
            | MS_RELATIME
            | linux_raw_sys::general::MS_REMOUNT);
    if flags & MS_NOSYMFOLLOW != 0 {
        statfs_flags |= ST_NOSYMFOLLOW;
    }
    statfs_flags
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FileHandleHeader {
    handle_bytes: u32,
    handle_type: i32,
}

pub(crate) fn encode_name_handle(
    stat: crate::file::Kstat,
) -> [u8; STARRY_FILE_HANDLE_BYTES as usize] {
    let mut handle = [0u8; STARRY_FILE_HANDLE_BYTES as usize];
    handle[..8].copy_from_slice(&stat.dev.to_ne_bytes());
    handle[8..].copy_from_slice(&stat.ino.to_ne_bytes());
    handle
}

pub fn sys_name_to_handle_at(
    dirfd: c_int,
    pathname: *const c_char,
    handle: *mut FileHandleHeader,
    mount_id: *mut c_int,
    flags: u32,
) -> AxResult<isize> {
    const VALID_FLAGS: u32 = AT_EMPTY_PATH | AT_SYMLINK_FOLLOW;

    debug!(
        "sys_name_to_handle_at <= dirfd: {dirfd}, pathname: {pathname:p}, handle: {handle:p}, \
         mount_id: {mount_id:p}, flags: {flags:#x}"
    );

    let supplied_size = unsafe { handle.vm_read_uninit()?.assume_init() }.handle_bytes;
    mount_id.vm_write(0)?;

    if flags & !VALID_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }

    let path = pathname.nullable().map(vm_load_string).transpose()?;
    let resolve_flags = if flags & AT_SYMLINK_FOLLOW != 0 {
        flags & AT_EMPTY_PATH
    } else {
        (flags & AT_EMPTY_PATH) | AT_SYMLINK_NOFOLLOW
    };
    let target = resolve_at(dirfd, path.as_deref(), resolve_flags)?;
    let stat = target.stat()?;
    let Some(loc) = target.into_file() else {
        return Err(AxError::OperationNotSupported);
    };

    if supplied_size > MAX_HANDLE_SZ {
        return Err(AxError::InvalidInput);
    }
    if supplied_size < STARRY_FILE_HANDLE_BYTES {
        handle.vm_write(FileHandleHeader {
            handle_bytes: STARRY_FILE_HANDLE_BYTES,
            handle_type: 0,
        })?;
        return Err(AxError::from(LinuxError::EOVERFLOW));
    }

    let encoded = encode_name_handle(stat);
    if let Ok(path) = loc.absolute_path() {
        HANDLE_RECORDS.write().insert(
            encoded,
            HandleRecord {
                path: path.to_string(),
                mount_id: loc.mountpoint().device(),
            },
        );
    }
    handle.vm_write(FileHandleHeader {
        handle_bytes: STARRY_FILE_HANDLE_BYTES,
        handle_type: STARRY_FILE_HANDLE_TYPE,
    })?;
    vm_write_slice(unsafe { handle.add(1).cast::<u8>() }, &encoded)?;
    mount_id.vm_write(loc.mountpoint().device() as c_int)?;
    Ok(0)
}

fn open_options_from_handle_flags(flags: u32) -> OpenOptions {
    let mut options = OpenOptions::new();
    match flags & O_ACCMODE {
        O_WRONLY => options.write(true),
        O_RDWR => options.read(true).write(true),
        _ => options.read(true),
    };
    if flags & O_PATH != 0 {
        options.path(true);
    }
    if flags & O_DIRECTORY != 0 {
        options.directory(true);
    }
    options
}

fn check_handle_mount(mount_fd: c_int, expected_mount_id: u64) -> AxResult {
    use linux_raw_sys::general::AT_FDCWD;

    if mount_fd == AT_FDCWD {
        return Ok(());
    }

    let _ = get_file_like(mount_fd)?;
    let Some(loc) = resolve_at(mount_fd, None, AT_EMPTY_PATH)?.into_file() else {
        return Err(AxError::from(LinuxError::ESTALE));
    };
    if loc.mountpoint().device() != expected_mount_id {
        return Err(AxError::from(LinuxError::ESTALE));
    }
    Ok(())
}

pub fn sys_open_by_handle_at(
    mount_fd: c_int,
    handle: *const FileHandleHeader,
    flags: u32,
) -> AxResult<isize> {
    debug!("sys_open_by_handle_at <= mount_fd: {mount_fd}, handle: {handle:p}, flags: {flags:#x}");

    if !current_has_cap(CAP_DAC_READ_SEARCH) {
        return Err(AxError::from(LinuxError::EPERM));
    }

    let header = unsafe { handle.vm_read_uninit()?.assume_init() };
    if header.handle_bytes == 0 || header.handle_bytes > MAX_HANDLE_SZ {
        return Err(AxError::InvalidInput);
    }
    if header.handle_bytes != STARRY_FILE_HANDLE_BYTES
        || header.handle_type != STARRY_FILE_HANDLE_TYPE
    {
        return Err(AxError::from(LinuxError::ESTALE));
    }

    let mut key = [MaybeUninit::<u8>::uninit(); STARRY_FILE_HANDLE_BYTES as usize];
    starry_vm::vm_read_slice(unsafe { handle.add(1).cast::<u8>() }, &mut key)?;
    let key = key.map(|byte| unsafe { byte.assume_init() });
    let record = HANDLE_RECORDS
        .read()
        .get(&key)
        .cloned()
        .ok_or_else(|| AxError::from(LinuxError::ESTALE))?;
    check_handle_mount(mount_fd, record.mount_id)?;

    let loc = resolve_at(
        linux_raw_sys::general::AT_FDCWD,
        Some(record.path.as_str()),
        AT_SYMLINK_NOFOLLOW,
    )?;
    if loc.stat()?.mode & S_IFMT == S_IFLNK {
        return Err(AxError::FilesystemLoop);
    }

    let options = open_options_from_handle_flags(flags);
    let f: Arc<dyn FileLike> = match options.open(&FS_CONTEXT.lock(), record.path.as_str())? {
        OpenResult::File(file) => Arc::new(File::new(file)),
        OpenResult::Dir(dir) => Arc::new(Directory::new(dir)),
    };
    if flags & O_NONBLOCK != 0 {
        f.set_nonblocking(true)?;
    }
    add_file_like(f, flags & O_CLOEXEC != 0).map(|fd| fd as isize)
}

pub fn sys_statx(
    dirfd: c_int,
    path: *const c_char,
    flags: u32,
    mask: u32,
    statxbuf: *mut statx,
) -> AxResult<isize> {
    // `statx()` uses pathname, dirfd, and flags to identify the target
    // file in one of the following ways:

    // An absolute pathname(situation 1)
    //        If pathname begins with a slash, then it is an absolute
    //        pathname that identifies the target file.  In this case,
    //        dirfd is ignored.

    // A relative pathname(situation 2)
    //        If pathname is a string that begins with a character other
    //        than a slash and dirfd is AT_FDCWD, then pathname is a
    //        relative pathname that is interpreted relative to the
    //        process's current working directory.

    // A directory-relative pathname(situation 3)
    //        If pathname is a string that begins with a character other
    //        than a slash and dirfd is a file descriptor that refers to
    //        a directory, then pathname is a relative pathname that is
    //        interpreted relative to the directory referred to by dirfd.
    //        (See openat(2) for an explanation of why this is useful.)

    // By file descriptor(situation 4)
    //        If pathname is an empty string (or NULL since Linux 6.11)
    //        and the AT_EMPTY_PATH flag is specified in flags (see
    //        below), then the target file is the one referred to by the
    //        file descriptor dirfd.

    let valid_flags = AT_EMPTY_PATH | AT_NO_AUTOMOUNT | AT_SYMLINK_NOFOLLOW | AT_STATX_SYNC_TYPE;
    let sync_flags = flags & AT_STATX_SYNC_TYPE;
    if flags & !valid_flags != 0
        || !matches!(sync_flags, 0 | AT_STATX_FORCE_SYNC | AT_STATX_DONT_SYNC)
        || mask & STATX__RESERVED != 0
    {
        return Err(AxError::InvalidInput);
    }

    let path = path.nullable().map(vm_load_string).transpose()?;
    debug!("sys_statx <= dirfd: {dirfd}, path: {path:?}, flags: {flags}");

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    check_parent_search_access(
        dirfd,
        path.as_deref(),
        flags,
        proc_data.euid(),
        proc_data.egid(),
    )?;

    statxbuf.vm_write(resolve_at(dirfd, path.as_deref(), flags)?.stat()?.into())?;

    Ok(0)
}

#[cfg(target_arch = "x86_64")]
pub fn sys_access(path: *const c_char, mode: u32) -> AxResult<isize> {
    use linux_raw_sys::general::AT_FDCWD;

    sys_faccessat2(AT_FDCWD, path, mode, 0)
}

pub fn sys_faccessat2(
    mut dirfd: c_int,
    path: *const c_char,
    mode: u32,
    flags: u32,
) -> AxResult<isize> {
    let path = path.nullable().map(vm_load_string).transpose()?;
    debug!("sys_faccessat2 <= dirfd: {dirfd}, path: {path:?}, mode: {mode}, flags: {flags}");

    let supported_flags = AT_EACCESS | AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH;
    if flags & !supported_flags != 0 {
        return Err(AxError::InvalidInput);
    }
    if mode & !(R_OK | W_OK | X_OK) != 0 {
        return Err(AxError::InvalidInput);
    }
    if path.as_deref().is_some_and(|path| path.starts_with('/')) {
        dirfd = AT_FDCWD;
    }

    let file = resolve_at(dirfd, path.as_deref(), flags)?;
    let stat = file.stat()?;
    let loc = file.into_file();
    if let Some(loc) = loc.as_ref() {
        note_mount_access(loc);
    }
    let read_only_fs =
        mode & W_OK != 0 && loc.is_some_and(|loc| loc.mount_flags() & MS_RDONLY != 0);

    let (uid, gid) = access_ids(flags);
    check_parent_search_access(dirfd, path.as_deref(), flags, uid, gid)?;

    if read_only_fs {
        return Err(AxError::ReadOnlyFilesystem);
    }
    if !has_access(mode, stat.mode, stat.uid, stat.gid, uid, gid) {
        return Err(AxError::PermissionDenied);
    }

    Ok(0)
}

fn access_ids(flags: u32) -> (u32, u32) {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    if flags & AT_EACCESS != 0 {
        (proc_data.euid(), proc_data.egid())
    } else {
        (proc_data.uid(), proc_data.gid())
    }
}

pub(super) fn check_parent_search_access(
    mut dirfd: c_int,
    path: Option<&str>,
    flags: u32,
    uid: u32,
    gid: u32,
) -> AxResult<()> {
    let Some(path) = path else {
        return Ok(());
    };
    if path.is_empty() || uid == 0 {
        return Ok(());
    }
    if path.starts_with('/') {
        dirfd = AT_FDCWD;
    }

    if needs_base_dir_search(path) {
        let base = if dirfd == AT_FDCWD {
            FS_CONTEXT.lock().current_dir().clone()
        } else {
            Directory::from_fd(dirfd)?.inner().clone()
        };
        check_search_access(&base, uid, gid)?;
    }

    let parent_flags = flags & !AT_SYMLINK_NOFOLLOW;
    let bytes = path.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b'/' || index == 0 || index + 1 == bytes.len() {
            continue;
        }
        if bytes[index - 1] == b'/' {
            continue;
        }

        let parent = &path[..index];
        if parent == "." {
            continue;
        }

        let parent = resolve_at(dirfd, Some(parent), parent_flags)?
            .into_file()
            .ok_or(AxError::NotADirectory)?;
        check_search_access(&parent, uid, gid)?;
    }

    Ok(())
}

fn needs_base_dir_search(path: &str) -> bool {
    !path.starts_with('/')
        && path
            .split('/')
            .any(|component| !component.is_empty() && component != ".")
}

fn check_search_access(dir: &Location, uid: u32, gid: u32) -> AxResult<()> {
    let metadata = dir.metadata()?;
    if has_access(
        X_OK,
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

fn has_access(mode: u32, st_mode: u32, st_uid: u32, st_gid: u32, uid: u32, gid: u32) -> bool {
    const OWNER_SHIFT: u32 = 6;
    const GROUP_SHIFT: u32 = 3;
    const OTHER_SHIFT: u32 = 0;
    const EXEC_MASK: u32 = 0o111;

    if mode == 0 {
        return true;
    }

    if uid == 0 {
        return mode & X_OK == 0 || st_mode & EXEC_MASK != 0;
    }

    let shift = if uid == st_uid {
        OWNER_SHIFT
    } else if gid == st_gid {
        GROUP_SHIFT
    } else {
        OTHER_SHIFT
    };
    let mut required = 0;
    if mode & R_OK != 0 {
        required |= 0o4 << shift;
    }
    if mode & W_OK != 0 {
        required |= 0o2 << shift;
    }
    if mode & X_OK != 0 {
        required |= 0o1 << shift;
    }

    st_mode & required == required
}

fn statfs(loc: &Location) -> AxResult<statfs> {
    let stat = loc.filesystem().stat()?;
    // FIXME: Zeroable
    let mut result: statfs = unsafe { core::mem::zeroed() };
    result.f_type = stat.fs_type as _;
    result.f_bsize = stat.block_size as _;
    result.f_blocks = stat.blocks as _;
    result.f_bfree = stat.blocks_free as _;
    result.f_bavail = stat.blocks_available as _;
    result.f_files = stat.file_count as _;
    result.f_ffree = stat.free_file_count as _;
    // TODO: fsid
    result.f_fsid = __kernel_fsid_t {
        val: [0, loc.mountpoint().device() as _],
    };
    result.f_namelen = stat.name_length as _;
    result.f_frsize = stat.fragment_size as _;
    result.f_flags = mount_flags_to_statfs_flags(stat.mount_flags | loc.mount_flags()) as _;
    Ok(result)
}

fn pseudo_statfs(fs_type: u32) -> statfs {
    // FIXME: Zeroable
    let mut result: statfs = unsafe { core::mem::zeroed() };
    result.f_type = fs_type as _;
    result.f_bsize = 4096;
    result.f_namelen = 255;
    result.f_frsize = 4096;
    result
}

pub fn sys_statfs(path: *const c_char, buf: *mut statfs) -> AxResult<isize> {
    let path = vm_load_string(path)?;
    debug!("sys_statfs <= path: {path:?}");

    buf.vm_write(statfs(
        &FS_CONTEXT
            .lock()
            .resolve(path)?
            .mountpoint()
            .root_location(),
    )?)?;
    Ok(0)
}

pub fn sys_fstatfs(fd: i32, buf: *mut statfs) -> AxResult<isize> {
    debug!("sys_fstatfs <= fd: {fd}");

    const PIPEFS_MAGIC: u32 = 0x5049_5045;
    const ANON_INODE_FS_MAGIC: u32 = 0x0904_1934;

    let file = get_file_like(fd)?;
    let stat = if let Some(file) = file.downcast_ref::<File>() {
        statfs(file.inner().location())?
    } else if let Some(dir) = file.downcast_ref::<Directory>() {
        statfs(dir.inner())?
    } else if file.downcast_ref::<Pipe>().is_some() {
        pseudo_statfs(PIPEFS_MAGIC)
    } else {
        pseudo_statfs(ANON_INODE_FS_MAGIC)
    };

    buf.vm_write(stat)?;
    Ok(0)
}
