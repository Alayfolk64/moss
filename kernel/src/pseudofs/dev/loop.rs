use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::FileBackend;
use axfs_ng_vfs::{DeviceId, NodeFlags, VfsResult};
use axsync::Mutex;
use linux_raw_sys::{
    ioctl::{
        BLKALIGNOFF, BLKBSZGET, BLKBSZSET, BLKDISCARD, BLKDISCARDZEROES, BLKFLSBUF, BLKGETSIZE,
        BLKGETSIZE64, BLKIOMIN, BLKIOOPT, BLKPBSZGET, BLKRAGET, BLKRASET, BLKROGET, BLKROSET,
        BLKROTATIONAL, BLKSECDISCARD, BLKSSZGET, BLKZEROOUT, FDGETPRM,
    },
    loop_device::{
        LOOP_CLR_FD, LOOP_GET_STATUS, LOOP_GET_STATUS64, LOOP_SET_BLOCK_SIZE, LOOP_SET_CAPACITY,
        LOOP_SET_FD, LOOP_SET_STATUS, LOOP_SET_STATUS64, loop_info, loop_info64,
    },
};
use starry_vm::{VmMutPtr, VmPtr};

use crate::{
    file::get_file_like,
    pseudofs::{DeviceMmap, DeviceOps},
};

const HDIO_GETGEO: u32 = 0x0301;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HdGeometry {
    heads: u8,
    sectors: u8,
    cylinders: u16,
    start: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FloppyStruct {
    size: u32,
    sect: u32,
    head: u32,
    track: u32,
    stretch: u32,
    gap: u8,
    rate: u8,
    spec1: u8,
    fmt_gap: u8,
    name: usize,
}

/// /dev/loopX devices
pub struct LoopDevice {
    number: u32,
    dev_id: DeviceId,
    /// Underlying file for the loop device, if any.
    pub file: Mutex<Option<FileBackend>>,
    /// Read-only flag for the loop device.
    pub ro: AtomicBool,
    /// Read-ahead size for the loop device, in bytes.
    pub ra: AtomicU32,
}

impl LoopDevice {
    pub(crate) fn new(number: u32, dev_id: DeviceId) -> Self {
        Self {
            number,
            dev_id,
            file: Mutex::new(None),
            ro: AtomicBool::new(false),
            ra: AtomicU32::new(512),
        }
    }

    /// Get information about the loop device.
    pub fn get_info(&self) -> AxResult<loop_info> {
        if self.file.lock().is_none() {
            return Err(AxError::from(LinuxError::ENXIO));
        }
        let mut res: loop_info = unsafe { core::mem::zeroed() };
        res.lo_number = self.number as _;
        res.lo_rdevice = self.dev_id.0 as _;
        Ok(res)
    }

    /// Set information for the loop device.
    pub fn set_info(&self, _src: loop_info) -> AxResult<()> {
        Ok(())
    }

    /// Get 64-bit loop device information.
    pub fn get_info64(&self) -> AxResult<loop_info64> {
        if self.file.lock().is_none() {
            return Err(AxError::from(LinuxError::ENXIO));
        }
        let mut res: loop_info64 = unsafe { core::mem::zeroed() };
        res.lo_number = self.number;
        res.lo_rdevice = self.dev_id.0 as _;
        Ok(res)
    }

    /// Set 64-bit loop device information.
    pub fn set_info64(&self, _src: loop_info64) -> AxResult<()> {
        Ok(())
    }

    /// Clone the underlying file of the loop device.
    pub fn clone_file(&self) -> VfsResult<FileBackend> {
        let file = self.file.lock().clone();
        file.ok_or(AxError::from(LinuxError::ENXIO))
    }

    fn geometry(&self) -> VfsResult<(u64, u32, u32, u32)> {
        let sectors = self.len()? / 512;
        let heads = 64;
        let sectors_per_track = 32;
        let cylinders = sectors
            .div_ceil((heads * sectors_per_track) as u64)
            .clamp(1, u16::MAX as u64) as u32;
        Ok((sectors, heads, sectors_per_track, cylinders))
    }
}

impl DeviceOps for LoopDevice {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        let file = self.file.lock().clone();
        file.ok_or(AxError::OperationNotPermitted)?
            .read_at(buf, offset)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> VfsResult<usize> {
        if self.ro.load(Ordering::Relaxed) {
            return Err(AxError::ReadOnlyFilesystem);
        }
        let file = self.file.lock().clone();
        file.ok_or(AxError::OperationNotPermitted)?
            .write_at(buf, offset)
    }

    fn len(&self) -> VfsResult<u64> {
        self.clone_file()?.location().len()
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> VfsResult<usize> {
        match cmd {
            LOOP_SET_FD => {
                let fd = arg as i32;
                if fd < 0 {
                    return Err(AxError::BadFileDescriptor);
                }
                let f = get_file_like(fd)?;
                let Some(file) = f.downcast_ref::<crate::file::File>() else {
                    return Err(AxError::InvalidInput);
                };
                let mut guard = self.file.lock();
                if guard.is_some() {
                    return Err(AxError::ResourceBusy);
                }

                *guard = Some(file.inner().backend()?.clone());
            }
            LOOP_CLR_FD => {
                let mut guard = self.file.lock();
                if guard.is_none() {
                    return Err(AxError::from(LinuxError::ENXIO));
                }
                *guard = None;
            }
            LOOP_GET_STATUS => {
                (arg as *mut loop_info).vm_write(self.get_info()?)?;
            }
            LOOP_GET_STATUS64 => {
                (arg as *mut loop_info64).vm_write(self.get_info64()?)?;
            }
            LOOP_SET_STATUS => {
                // FIXME: AnyBitPattern
                let info = unsafe { (arg as *const loop_info).vm_read_uninit()?.assume_init() };
                self.set_info(info)?;
            }
            LOOP_SET_STATUS64 => {
                let info = unsafe { (arg as *const loop_info64).vm_read_uninit()?.assume_init() };
                self.set_info64(info)?;
            }
            LOOP_SET_CAPACITY | LOOP_SET_BLOCK_SIZE => {}
            // TODO: the following should apply to any block devices
            BLKGETSIZE | BLKGETSIZE64 => {
                let file = self.clone_file()?;
                let sectors = file.location().len()? / 512;
                if cmd == BLKGETSIZE {
                    (arg as *mut u32).vm_write(sectors as _)?;
                } else {
                    (arg as *mut u64).vm_write(sectors * 512)?;
                }
            }
            BLKROGET => {
                (arg as *mut u32).vm_write(self.ro.load(Ordering::Relaxed) as u32)?;
            }
            BLKROSET => {
                let ro = (arg as *const u32).vm_read()?;
                if ro != 0 && ro != 1 {
                    return Err(AxError::InvalidInput);
                }
                self.ro.store(ro != 0, Ordering::Relaxed);
            }
            BLKSSZGET | BLKPBSZGET | BLKBSZGET => {
                (arg as *mut u32).vm_write(512)?;
            }
            BLKIOMIN | BLKIOOPT => {
                (arg as *mut u32).vm_write(512)?;
            }
            BLKALIGNOFF | BLKDISCARDZEROES | BLKROTATIONAL => {
                (arg as *mut u32).vm_write(0)?;
            }
            BLKBSZSET => {
                let size = (arg as *const u32).vm_read()?;
                if size == 0 || size > 4096 || !size.is_power_of_two() {
                    return Err(AxError::InvalidInput);
                }
            }
            BLKFLSBUF | BLKDISCARD | BLKSECDISCARD | BLKZEROOUT => {}
            HDIO_GETGEO => {
                let (_, heads, sectors_per_track, cylinders) = self.geometry()?;
                (arg as *mut HdGeometry).vm_write(HdGeometry {
                    heads: heads as u8,
                    sectors: sectors_per_track as u8,
                    cylinders: cylinders as u16,
                    start: 0,
                })?;
            }
            FDGETPRM => {
                let (sectors, heads, sectors_per_track, cylinders) = self.geometry()?;
                (arg as *mut FloppyStruct).vm_write(FloppyStruct {
                    size: sectors.min(u32::MAX as u64) as u32,
                    sect: sectors_per_track,
                    head: heads,
                    track: cylinders,
                    ..Default::default()
                })?;
            }
            BLKRAGET => {
                (arg as *mut usize).vm_write(self.ra.load(Ordering::Relaxed) as usize)?;
            }
            BLKRASET => {
                self.ra.store(arg as u32, Ordering::Relaxed);
            }
            _ => {
                warn!("unknown ioctl for loop device: {cmd}");
                return Err(AxError::NotATty);
            }
        }
        Ok(0)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn mmap(&self) -> DeviceMmap {
        if let Some(FileBackend::Cached(cache)) = self.file.lock().as_ref() {
            DeviceMmap::Cache(cache.clone())
        } else {
            DeviceMmap::None
        }
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE
    }
}
