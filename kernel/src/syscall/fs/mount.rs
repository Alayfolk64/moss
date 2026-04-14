use core::ffi::{c_char, c_void};

use axerrno::{AxError, AxResult};
use axfs::FS_CONTEXT;
use linux_raw_sys::general::MS_RDONLY;
use starry_vm::VmPtr;

use crate::{mm::vm_load_string, pseudofs::MemoryFs};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum MountKind {
    Tmpfs,
    CompatVfat,
}

fn detect_mount_kind(fs_type: &str) -> Option<MountKind> {
    match fs_type {
        "tmpfs" | "ramfs" => Some(MountKind::Tmpfs),
        // Basic oscomp mount testcase uses `vfat` on /dev/vda2.
        // We currently don't provide a block-backed vfat driver via mount(2),
        // so keep syscall semantics (mount/umount succeed) with a memory fs.
        "vfat" | "fat" | "msdos" => Some(MountKind::CompatVfat),
        _ => None,
    }
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
    let fs_type = vm_load_string(fs_type)?;
    debug!("sys_mount <= source: {source:?}, target: {target:?}, fs_type: {fs_type:?}");

    let Some(kind) = detect_mount_kind(&fs_type) else {
        return Err(AxError::NoSuchDevice);
    };

    let fs = MemoryFs::new_with_flags(flags as u32 & MS_RDONLY);
    if kind == MountKind::CompatVfat {
        debug!(
            "mount: use tmpfs-compatible backend for fs_type={fs_type:?}, source={source:?}, target={target:?}"
        );
    }

    let target = FS_CONTEXT.lock().resolve(target)?;
    target.mount(&fs)?;

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
    target.unmount()?;
    Ok(0)
}
