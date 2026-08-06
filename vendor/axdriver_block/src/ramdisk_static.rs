//! A RAM disk driver backed by a static slice.

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use spin::Mutex;

use crate::{BlockDriverOps, block_range::checked_block_range};

const BLOCK_SIZE: usize = 512;

/// A RAM disk backed by a static slice.
pub struct RamDisk {
    data: Mutex<&'static mut [u8]>,
}

impl Default for RamDisk {
    fn default() -> Self {
        Self {
            data: Mutex::new(&mut []),
        }
    }
}

impl RamDisk {
    /// Creates a new RAM disk from the given static buffer.
    ///
    /// # Panics
    /// Panics if the buffer is not aligned to block size or its size is not
    /// a multiple of block size.
    pub fn new(buf: &'static mut [u8]) -> Self {
        assert!(buf.as_ptr().addr() & (BLOCK_SIZE - 1) == 0);
        assert!(buf.len() % BLOCK_SIZE == 0);
        RamDisk {
            data: Mutex::new(buf),
        }
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
        (self.data.lock().len() / BLOCK_SIZE) as u64
    }

    #[inline]
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> DevResult {
        if buf.len() % BLOCK_SIZE != 0 {
            return Err(DevError::InvalidParam);
        }
        let data = self.data.lock();
        let range =
            checked_block_range(block_id, buf.len(), BLOCK_SIZE, data.len()).ok_or(DevError::Io)?;
        buf.copy_from_slice(&data[range]);
        Ok(())
    }

    fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult {
        if buf.len() % BLOCK_SIZE != 0 {
            return Err(DevError::InvalidParam);
        }
        let mut data = self.data.lock();
        let range =
            checked_block_range(block_id, buf.len(), BLOCK_SIZE, data.len()).ok_or(DevError::Io)?;
        data[range].copy_from_slice(buf);
        Ok(())
    }

    fn flush(&self) -> DevResult {
        Ok(())
    }
}
