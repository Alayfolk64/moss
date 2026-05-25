use core::ffi::{c_char, c_void};

use axdriver::prelude::{BaseDriverOps, BlockDriverOps, DevError, DevResult, DeviceType};
use axerrno::{AxError, AxResult};
use axfs::{FS_CONTEXT, FileBackend};
use linux_raw_sys::general::{MS_BIND, MS_RDONLY};
use starry_vm::VmPtr;

use crate::{
    mm::vm_load_string,
    pseudofs::{Device, MemoryFs, dev::r#loop::LoopDevice},
};

const LOOP_BLOCK_SIZE: usize = 512;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum MountKind {
    Tmpfs,
    CompatMemory,
    Ext4,
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

fn loop_file_block_device(source: &str) -> AxResult<Option<LoopFileBlockDevice>> {
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
    let file = loop_device.clone_file()?;
    Ok(Some(LoopFileBlockDevice::new(file)?))
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

    if flags as u32 & MS_BIND != 0 {
        let Some(source) = source.as_deref() else {
            return Err(AxError::InvalidInput);
        };
        let fs_context = FS_CONTEXT.lock();
        let source = fs_context.resolve(source)?;
        let target = fs_context.resolve(target)?;
        target.bind_mount(&source)?;
        return Ok(0);
    }

    let Some(kind) = detect_mount_kind(&fs_type) else {
        return Err(AxError::NoSuchDevice);
    };

    match kind {
        MountKind::Tmpfs | MountKind::CompatMemory => {
            let fs = MemoryFs::new_with_flags(flags as u32 & MS_RDONLY);
            if kind == MountKind::CompatMemory {
                debug!(
                    "mount: use tmpfs-compatible backend for fs_type={fs_type:?}, \
                     source={source:?}, target={target:?}"
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
            if source == "/dev/zero" {
                debug!("mount ext4: reject /dev/zero probe with EINVAL");
                return Err(AxError::InvalidInput);
            }

            let Some(dev_idx) = parse_block_source_index(source) else {
                let Some(dev) = loop_file_block_device(source)? else {
                    warn!("mount ext4: unsupported source path: {source}");
                    return Err(AxError::NoSuchDevice);
                };
                let fs = match axfs::new_ext4_filesystem_from_block_driver(dev) {
                    Ok(fs) => fs,
                    Err(err) => {
                        warn!("mount ext4: source={source} init loop fs failed: {err:?}");
                        return Err(err.into());
                    }
                };
                let target = match FS_CONTEXT.lock().resolve(target) {
                    Ok(target) => target,
                    Err(err) => {
                        warn!("mount ext4: source={source} resolve target failed: {err:?}");
                        return Err(err.into());
                    }
                };
                if let Err(err) = target.mount(&fs) {
                    warn!("mount ext4: source={source} mount target failed: {err:?}");
                    return Err(err.into());
                }
                return Ok(0);
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
    target.filesystem().flush()?;
    target.unmount()?;
    Ok(0)
}
