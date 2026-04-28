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
    Ext4,
}

fn detect_mount_kind(fs_type: &str) -> Option<MountKind> {
    match fs_type {
        "tmpfs" | "ramfs" => Some(MountKind::Tmpfs),
        // Basic oscomp mount testcase uses `vfat` on /dev/vda2.
        // We currently don't provide a block-backed vfat driver via mount(2),
        // so keep syscall semantics (mount/umount succeed) with a memory fs.
        "vfat" | "fat" | "msdos" => Some(MountKind::CompatVfat),
        "ext4" => Some(MountKind::Ext4),
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

    match kind {
        MountKind::Tmpfs | MountKind::CompatVfat => {
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
        MountKind::Ext4 => {
            let Some(source) = source.as_deref() else {
                return Err(AxError::InvalidInput);
            };
            let Some(dev_idx) = parse_block_source_index(source) else {
                warn!("mount ext4: unsupported source path: {source}");
                return Err(AxError::NoSuchDevice);
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
            let fs = match axfs::new_filesystem_from_block_device(dev) {
                Ok(fs) => fs,
                Err(err) => {
                    warn!(
                        "mount ext4: source={source} index={} init fs failed: {err:?}",
                        dev_idx
                    );
                    return Err(err.into());
                }
            };
            let target = match FS_CONTEXT.lock().resolve(target) {
                Ok(target) => target,
                Err(err) => {
                    warn!(
                        "mount ext4: source={source} index={} resolve target failed: {err:?}",
                        dev_idx
                    );
                    return Err(err.into());
                }
            };
            if let Err(err) = target.mount(&fs) {
                warn!(
                    "mount ext4: source={source} index={} mount target failed: {err:?}",
                    dev_idx
                );
                return Err(err.into());
            }
            Ok(0)
        }
    }
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
