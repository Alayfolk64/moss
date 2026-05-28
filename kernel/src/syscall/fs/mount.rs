use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::{
    ffi::{c_char, c_int, c_void},
    sync::atomic::Ordering,
    task::Context,
};

use axdriver::prelude::{BaseDriverOps, BlockDriverOps, DevError, DevResult, DeviceType};
use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, FileBackend};
use axfs_ng_vfs::Filesystem;
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
use axtask::current;
use linux_raw_sys::general::{
    AT_FDCWD, MS_BIND, MS_MOVE, MS_NOATIME, MS_NODEV, MS_NODIRATIME, MS_NOEXEC, MS_NOSUID,
    MS_NOSYMFOLLOW, MS_PRIVATE, MS_RDONLY, MS_REMOUNT, MS_STRICTATIME,
};
use spin::RwLock;
use starry_vm::VmPtr;

use crate::{
    file::{FileLike, add_file_like, get_file_like, with_fs},
    mm::vm_load_string,
    pseudofs::{Device, MemoryFs, dev::r#loop::LoopDevice},
    syscall::fs::inotify::notify_inotify_unmount_dev,
    task::AsThread,
};

const LOOP_BLOCK_SIZE: usize = 512;
const SUPPORTED_MOUNT_FLAGS: u32 = MS_RDONLY
    | MS_NODEV
    | MS_NOEXEC
    | MS_NOSUID
    | MS_NOATIME
    | MS_NODIRATIME
    | MS_REMOUNT
    | MS_NOSYMFOLLOW
    | MS_STRICTATIME;
const FSOPEN_CLOEXEC: u32 = 1;
const FSMOUNT_CLOEXEC: u32 = 1;
const FSCONFIG_SET_FLAG: u32 = 0;
const FSCONFIG_SET_STRING: u32 = 1;
const FSCONFIG_SET_BINARY: u32 = 2;
const FSCONFIG_SET_PATH: u32 = 3;
const FSCONFIG_SET_PATH_EMPTY: u32 = 4;
const FSCONFIG_SET_FD: u32 = 5;
const FSCONFIG_CMD_CREATE: u32 = 6;
const FSCONFIG_CMD_RECONFIGURE: u32 = 7;
const MOVE_MOUNT_F_EMPTY_PATH: u32 = 4;
const VALID_FSMOUNT_ATTRS: u32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x80;
static MOUNTED_PATHS: RwLock<Vec<String>> = RwLock::new(Vec::new());

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum MountKind {
    Tmpfs,
    CompatMemory,
    Ext4,
}

struct FsContextFd {
    fs_type: String,
    source: Mutex<Option<String>>,
    created: Mutex<bool>,
}

struct DetachedMountFd {
    fs: Filesystem,
    mount_flags: u32,
}

impl FileLike for FsContextFd {
    fn path(&self) -> alloc::borrow::Cow<'_, str> {
        "anon_inode:[fscontext]".into()
    }
}

impl Pollable for FsContextFd {
    fn poll(&self) -> IoEvents {
        IoEvents::empty()
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

impl FileLike for DetachedMountFd {
    fn path(&self) -> alloc::borrow::Cow<'_, str> {
        "anon_inode:[fsmount]".into()
    }
}

impl Pollable for DetachedMountFd {
    fn poll(&self) -> IoEvents {
        IoEvents::empty()
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

fn detect_mount_kind(fs_type: &str) -> Option<MountKind> {
    match fs_type {
        "tmpfs" | "ramfs" => Some(MountKind::Tmpfs),
        // LTP fanotify's all-filesystems matrix formats a loop device before
        // mounting. For these filesystems we currently keep mount semantics
        // with an isolated in-memory VFS backend; the loop device still exists
        // so mkfs can probe and prepare the image.
        "xfs" | "btrfs" | "vfat" | "fat" | "msdos" | "exfat" | "ntfs" => {
            Some(MountKind::CompatMemory)
        }
        "ext2" | "ext3" | "ext4" => Some(MountKind::Ext4),
        _ => None,
    }
}

fn parse_block_source_index(source: &str) -> Option<usize> {
    let name = source.strip_prefix("/dev/")?;
    let bytes = name.as_bytes();
    if name.len() >= 3 && (name.starts_with("vd") || name.starts_with("sd")) {
        let ch = bytes[2];
        let suffix = &name[3..];
        if ch.is_ascii_lowercase() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return Some((ch - b'a') as usize);
        }
    }
    None
}

fn parse_loop_source_index(source: &str) -> Option<u32> {
    let name = source.strip_prefix("/dev/")?;
    let suffix = name.strip_prefix("loop")?;
    if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    suffix.parse().ok()
}

struct LoopFileBlockDevice {
    file: FileBackend,
    blocks: u64,
}

impl LoopFileBlockDevice {
    fn new(file: FileBackend) -> AxResult<Self> {
        let blocks = file.location().len()? / LOOP_BLOCK_SIZE as u64;
        if blocks == 0 {
            return Err(AxError::InvalidInput);
        }
        Ok(Self { file, blocks })
    }

    fn checked_range(&self, block_id: u64, len: usize) -> DevResult<u64> {
        if len % LOOP_BLOCK_SIZE != 0 {
            return Err(DevError::InvalidParam);
        }
        let offset = block_id
            .checked_mul(LOOP_BLOCK_SIZE as u64)
            .ok_or(DevError::InvalidParam)?;
        let end = offset
            .checked_add(len as u64)
            .ok_or(DevError::InvalidParam)?;
        if end > self.blocks * LOOP_BLOCK_SIZE as u64 {
            return Err(DevError::Io);
        }
        Ok(offset)
    }
}

impl BaseDriverOps for LoopFileBlockDevice {
    fn device_name(&self) -> &str {
        "loop-file"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl BlockDriverOps for LoopFileBlockDevice {
    fn num_blocks(&self) -> u64 {
        self.blocks
    }

    fn block_size(&self) -> usize {
        LOOP_BLOCK_SIZE
    }

    fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> DevResult {
        let offset = self.checked_range(block_id, buf.len())?;
        let mut done = 0;
        while done < buf.len() {
            let read = self
                .file
                .read_at(&mut buf[done..], offset + done as u64)
                .map_err(|_| DevError::Io)?;
            if read == 0 {
                return Err(DevError::Io);
            }
            done += read;
        }
        Ok(())
    }

    fn write_block(&mut self, block_id: u64, buf: &[u8]) -> DevResult {
        let offset = self.checked_range(block_id, buf.len())?;
        let mut done = 0;
        while done < buf.len() {
            let written = self
                .file
                .write_at(&buf[done..], offset + done as u64)
                .map_err(|_| DevError::Io)?;
            if written == 0 {
                return Err(DevError::Io);
            }
            done += written;
        }
        Ok(())
    }

    fn flush(&mut self) -> DevResult {
        self.file.sync(false).map_err(|_| DevError::Io)
    }
}

fn loop_file_block_device(source: &str, mount_flags: u32) -> AxResult<Option<LoopFileBlockDevice>> {
    if parse_loop_source_index(source).is_none() {
        return Ok(None);
    }

    let source = FS_CONTEXT.lock().resolve(source)?;
    let device = source.entry().downcast::<Device>()?;
    let loop_device = device
        .inner()
        .as_any()
        .downcast_ref::<LoopDevice>()
        .ok_or(AxError::NoSuchDevice)?;
    if loop_device.ro.load(Ordering::Relaxed) && mount_flags & MS_RDONLY == 0 {
        return Err(AxError::PermissionDenied);
    }
    let file = loop_device.clone_file()?;
    Ok(Some(LoopFileBlockDevice::new(file)?))
}

fn create_filesystem(
    fs_type: &str,
    source: Option<&str>,
    mount_flags: u32,
) -> AxResult<Filesystem> {
    let Some(kind) = detect_mount_kind(fs_type) else {
        return Err(AxError::NoSuchDevice);
    };

    match kind {
        MountKind::Tmpfs | MountKind::CompatMemory => Ok(MemoryFs::new_with_flags(mount_flags)),
        MountKind::Ext4 => {
            let Some(source) = source else {
                return Err(AxError::InvalidInput);
            };
            if source == "/dev/zero" {
                return Err(AxError::InvalidInput);
            }

            let Some(dev_idx) = parse_block_source_index(source) else {
                if parse_loop_source_index(source).is_none()
                    && let Ok(loc) = FS_CONTEXT.lock().resolve(source)
                    && loc.metadata()?.node_type != axfs_ng_vfs::NodeType::BlockDevice
                {
                    return Err(AxError::from(LinuxError::ENOTBLK));
                }
                let Some(dev) = loop_file_block_device(source, mount_flags)? else {
                    warn!("mount ext4: unsupported source path: {source}");
                    return Err(AxError::NoSuchDevice);
                };
                return axfs::new_ext4_filesystem_from_block_driver(dev).map_err(|err| {
                    warn!("mount ext4: source={source} init loop fs failed: {err:?}");
                    err.into()
                });
            };

            let total_slots = axruntime::runtime_block_device_count();
            if dev_idx >= total_slots {
                warn!(
                    "mount ext4: source={source} index={} out of range (runtime_block_slots={})",
                    dev_idx, total_slots
                );
                return Err(AxError::NoSuchDevice);
            }

            let Some(dev) = axruntime::take_runtime_block_device(dev_idx) else {
                warn!(
                    "mount ext4: source={source} index={} is unavailable",
                    dev_idx
                );
                return Err(AxError::NoSuchDevice);
            };
            axfs::new_filesystem_from_block_device(dev).map_err(|err| {
                warn!(
                    "mount ext4: source={source} index={} init fs failed: {err:?}",
                    dev_idx
                );
                err.into()
            })
        }
    }
}

fn mount_filesystem_at(
    fs: Filesystem,
    mount_flags: u32,
    dirfd: c_int,
    target_path: &str,
) -> AxResult {
    let target = with_fs(dirfd, |fs_ctx| fs_ctx.resolve(target_path))?;
    let path = target
        .absolute_path()
        .map_or_else(|_| target_path.to_string(), |path| path.to_string());
    if MOUNTED_PATHS.read().iter().any(|it| it == &path) {
        return Err(AxError::ResourceBusy);
    }
    if let Err(err) = target.mount_with_flags(&fs, mount_flags) {
        warn!("mount target={target_path} failed: {err:?}");
        return Err(err.into());
    }
    let mut paths = MOUNTED_PATHS.write();
    if !paths.iter().any(|it| it == &path) {
        paths.push(path);
    }
    Ok(())
}

pub fn proc_mounts() -> String {
    let mut out = String::from("proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0\n");
    for path in MOUNTED_PATHS.read().iter() {
        out.push_str(&format!("none {path} auto rw 0 0\n"));
    }
    out
}

fn unregister_mount_path(path: &str) {
    MOUNTED_PATHS.write().retain(|it| it != path);
}

fn mount_flags_from_attrs(attrs: u32) -> u32 {
    if attrs & 0x1 != 0 { MS_RDONLY } else { 0 }
}

pub fn sys_fsopen(fs_name: *const c_char, flags: u32) -> AxResult<isize> {
    if flags & !FSOPEN_CLOEXEC != 0 {
        return Err(AxError::InvalidInput);
    }
    let fs_type = vm_load_string(fs_name)?;
    if detect_mount_kind(&fs_type).is_none() {
        return Err(AxError::NoSuchDevice);
    }
    let fd = Arc::new(FsContextFd {
        fs_type,
        source: Mutex::new(None),
        created: Mutex::new(false),
    });
    add_file_like(fd, flags & FSOPEN_CLOEXEC != 0).map(|fd| fd as _)
}

pub fn sys_fsconfig(
    fd: c_int,
    cmd: u32,
    key: *const c_char,
    value: *const c_void,
    aux: c_int,
) -> AxResult<isize> {
    if fd < 0 {
        return Err(AxError::InvalidInput);
    }
    let ctx = get_file_like(fd)
        .map_err(|_| AxError::InvalidInput)?
        .downcast_arc::<FsContextFd>()
        .map_err(|_| AxError::InvalidInput)?;
    match cmd {
        FSCONFIG_SET_FLAG => {
            if key.is_null() || !value.is_null() || aux != 0 {
                return Err(AxError::InvalidInput);
            }
            vm_load_string(key)?;
        }
        FSCONFIG_SET_STRING => {
            if key.is_null() || value.is_null() || aux != 0 {
                return Err(AxError::InvalidInput);
            }
            let key = vm_load_string(key)?;
            if key != "source" {
                return Err(AxError::InvalidInput);
            }
            *ctx.source.lock() = Some(vm_load_string(value.cast())?);
        }
        FSCONFIG_SET_PATH | FSCONFIG_SET_PATH_EMPTY => {
            if key.is_null() || value.is_null() || (aux < 0 && aux != AT_FDCWD) {
                return Err(AxError::InvalidInput);
            }
            vm_load_string(key)?;
            vm_load_string(value.cast())?;
        }
        FSCONFIG_SET_FD => {
            if key.is_null() || !value.is_null() || aux < 0 {
                return Err(AxError::InvalidInput);
            }
            vm_load_string(key)?;
        }
        FSCONFIG_SET_BINARY | FSCONFIG_CMD_RECONFIGURE => {
            return Err(AxError::InvalidInput);
        }
        FSCONFIG_CMD_CREATE => {
            if !key.is_null() || !value.is_null() || aux != 0 {
                return Err(AxError::InvalidInput);
            }
            *ctx.created.lock() = true;
        }
        _ => return Err(AxError::OperationNotSupported),
    }
    Ok(0)
}

pub fn sys_fsmount(fd: c_int, flags: u32, mount_attrs: u32) -> AxResult<isize> {
    if flags & !FSMOUNT_CLOEXEC != 0 || mount_attrs & !VALID_FSMOUNT_ATTRS != 0 {
        return Err(AxError::InvalidInput);
    }
    let ctx = get_file_like(fd)?
        .downcast_arc::<FsContextFd>()
        .map_err(|_| AxError::InvalidInput)?;
    if !*ctx.created.lock() {
        return Err(AxError::InvalidInput);
    }
    let mount_flags = mount_flags_from_attrs(mount_attrs);
    let source = ctx.source.lock().clone();
    let fs = create_filesystem(&ctx.fs_type, source.as_deref(), mount_flags)?;
    let mount = Arc::new(DetachedMountFd { fs, mount_flags });
    add_file_like(mount, flags & FSMOUNT_CLOEXEC != 0).map(|fd| fd as _)
}

pub fn sys_move_mount(
    from_dirfd: c_int,
    from_path: *const c_char,
    to_dirfd: c_int,
    to_path: *const c_char,
    flags: u32,
) -> AxResult<isize> {
    let from_path = vm_load_string(from_path)?;
    if flags != MOVE_MOUNT_F_EMPTY_PATH || !from_path.is_empty() {
        return Err(AxError::InvalidInput);
    }
    let to_path = vm_load_string(to_path)?;
    let mount = get_file_like(from_dirfd)?
        .downcast_arc::<DetachedMountFd>()
        .map_err(|_| AxError::InvalidInput)?;
    mount_filesystem_at(mount.fs.clone(), mount.mount_flags, to_dirfd, &to_path)?;
    Ok(0)
}

pub fn sys_mount(
    source: *const c_char,
    target: *const c_char,
    fs_type: *const c_char,
    flags: i32,
    _data: *const c_void,
) -> AxResult<isize> {
    let source = source.nullable().map(vm_load_string).transpose()?;
    let target = vm_load_string(target)?;
    let fs_type = fs_type.nullable().map(vm_load_string).transpose()?;
    debug!("sys_mount <= source: {source:?}, target: {target:?}, fs_type: {fs_type:?}");

    if current().as_thread().proc_data.euid() != 0 {
        return Err(AxError::OperationNotPermitted);
    }

    let mount_flags = flags as u32 & SUPPORTED_MOUNT_FLAGS;

    if flags as u32 & MS_BIND != 0 {
        let Some(source) = source.as_deref() else {
            return Err(AxError::InvalidInput);
        };
        let fs_context = FS_CONTEXT.lock();
        let source = fs_context.resolve(source)?;
        let target = fs_context.resolve(target.as_str())?;
        let path = target
            .absolute_path()
            .map_or_else(|_| target.name().to_string(), |path| path.to_string());
        target.bind_mount(&source)?;
        let mut paths = MOUNTED_PATHS.write();
        if !paths.iter().any(|it| it == &path) {
            paths.push(path);
        }
        return Ok(0);
    }

    if flags as u32 & MS_PRIVATE != 0 {
        FS_CONTEXT.lock().resolve(target.as_str())?;
        return Ok(0);
    }

    if flags as u32 & MS_MOVE != 0 {
        let Some(source) = source.as_deref() else {
            return Err(AxError::InvalidInput);
        };
        let fs_context = FS_CONTEXT.lock();
        let source = fs_context.resolve(source)?;
        let target = fs_context.resolve(target.as_str())?;
        let source_path = source.absolute_path().ok().map(|path| path.to_string());
        let target_path = target
            .absolute_path()
            .map_or_else(|_| target.name().to_string(), |path| path.to_string());
        target.bind_mount(&source)?;
        source.unmount()?;
        if let Some(source_path) = source_path {
            unregister_mount_path(&source_path);
        }
        let mut paths = MOUNTED_PATHS.write();
        if !paths.iter().any(|it| it == &target_path) {
            paths.push(target_path);
        }
        return Ok(0);
    }

    if flags as u32 & MS_REMOUNT != 0 {
        let target = FS_CONTEXT.lock().resolve(target.as_str())?;
        if !target.is_root_of_mount() {
            return Err(AxError::InvalidInput);
        }
        if mount_flags & MS_RDONLY != 0 {
            return Err(AxError::ResourceBusy);
        }
        target.mountpoint().set_mount_flags(mount_flags);
        return Ok(0);
    }

    let Some(fs_type) = fs_type.as_deref() else {
        return Err(AxError::InvalidInput);
    };
    let fs = create_filesystem(fs_type, source.as_deref(), mount_flags)?;
    mount_filesystem_at(fs, mount_flags, AT_FDCWD, &target)?;
    Ok(0)
}

pub fn sys_umount2(target: *const c_char, flags: i32) -> AxResult<isize> {
    const MNT_FORCE: i32 = 0x1;
    const MNT_DETACH: i32 = 0x2;
    const MNT_EXPIRE: i32 = 0x4;
    const UMOUNT_NOFOLLOW: i32 = 0x8;
    const ALLOWED_FLAGS: i32 = MNT_FORCE | MNT_DETACH | MNT_EXPIRE | UMOUNT_NOFOLLOW;

    if flags & !ALLOWED_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }

    let target = vm_load_string(target)?;
    debug!("sys_umount2 <= target: {target:?}, flags: {flags:#x}");
    let target = FS_CONTEXT.lock().resolve(target)?;
    let path = target.absolute_path().ok().map(|path| path.to_string());
    let dev = target.metadata()?.device;
    target.filesystem().flush()?;
    target.unmount()?;
    if let Some(path) = path {
        unregister_mount_path(&path);
    }
    notify_inotify_unmount_dev(dev);
    Ok(0)
}
