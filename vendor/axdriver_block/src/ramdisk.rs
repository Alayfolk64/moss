//! A RAM disk driver backed by heap memory.

extern crate alloc;

use alloc::alloc::{alloc_zeroed, dealloc, handle_alloc_error};
use core::{alloc::Layout, ptr::NonNull};

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use spin::Mutex;

use crate::{BlockDriverOps, block_range::checked_block_range};

const BLOCK_SIZE: usize = 512;

/// A RAM disk backed by heap memory.
pub struct RamDisk {
    data: NonNull<[u8]>,
    io_lock: Mutex<()>,
}

unsafe impl Send for RamDisk {}
unsafe impl Sync for RamDisk {}

impl Default for RamDisk {
    fn default() -> Self {
        Self {
            data: NonNull::<[u8; 0]>::dangling(),
            io_lock: Mutex::new(()),
        }
    }
}

impl RamDisk {
    /// Creates a new RAM disk with the given size hint.
    ///
    /// The actual size of the RAM disk will be aligned upwards to the block
    /// size (512 bytes).
    pub fn new(size_hint: usize) -> Self {
        let size = align_up(size_hint).expect("RAM disk size overflow");
        if size == 0 {
            return Self::default();
        }
        let layout = Layout::from_size_align(size, BLOCK_SIZE).unwrap();
        let ptr = NonNull::new(unsafe { alloc_zeroed(layout) })
            .unwrap_or_else(|| handle_alloc_error(layout));
        Self {
            data: NonNull::slice_from_raw_parts(ptr, size),
            io_lock: Mutex::new(()),
        }
    }
}

impl Drop for RamDisk {
    fn drop(&mut self) {
        if self.data.is_empty() {
            return;
        }
        unsafe {
            dealloc(
                self.data.cast::<u8>().as_ptr(),
                Layout::from_size_align_unchecked(self.data.len(), BLOCK_SIZE),
            );
        }
    }
}

impl From<&[u8]> for RamDisk {
    fn from(data: &[u8]) -> Self {
        let this = RamDisk::new(data.len());
        unsafe {
            this.data
                .as_ptr()
                .cast::<u8>()
                .copy_from_nonoverlapping(data.as_ptr(), data.len())
        };
        this
    }
}

impl BaseDriverOps for RamDisk {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn device_name(&self) -> &str {
        "ramdisk"
    }
}

impl BlockDriverOps for RamDisk {
    #[inline]
    fn num_blocks(&self) -> u64 {
        (self.data.len() / BLOCK_SIZE) as u64
    }

    #[inline]
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> DevResult {
        let _guard = self.io_lock.lock();
        if buf.len() % BLOCK_SIZE != 0 {
            return Err(DevError::InvalidParam);
        }
        let range = checked_block_range(block_id, buf.len(), BLOCK_SIZE, self.data.len())
            .ok_or(DevError::Io)?;
        unsafe {
            self.data
                .as_ptr()
                .cast::<u8>()
                .add(range.start)
                .copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len())
        };
        Ok(())
    }

    fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult {
        let _guard = self.io_lock.lock();
        if buf.len() % BLOCK_SIZE != 0 {
            return Err(DevError::InvalidParam);
        }
        let range = checked_block_range(block_id, buf.len(), BLOCK_SIZE, self.data.len())
            .ok_or(DevError::Io)?;
        unsafe {
            self.data
                .as_ptr()
                .cast::<u8>()
                .add(range.start)
                .copy_from_nonoverlapping(buf.as_ptr(), buf.len())
        };
        Ok(())
    }

    fn flush(&self) -> DevResult {
        Ok(())
    }
}

const fn align_up(val: usize) -> Option<usize> {
    match val.checked_add(BLOCK_SIZE - 1) {
        Some(value) => Some(value & !(BLOCK_SIZE - 1)),
        None => None,
    }
}
