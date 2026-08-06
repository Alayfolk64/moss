use axdriver_base::{BaseDriverOps, DevResult, DeviceType};
use axdriver_block::BlockDriverOps;
use spin::Mutex;
use virtio_drivers::{Hal, device::blk::VirtIOBlk as InnerDev, transport::Transport};

use crate::as_dev_err;

/// The VirtIO block device driver.
pub struct VirtIoBlkDev<H: Hal, T: Transport> {
    inner: Mutex<InnerDev<H, T>>,
}

unsafe impl<H: Hal, T: Transport> Send for VirtIoBlkDev<H, T> {}
unsafe impl<H: Hal, T: Transport> Sync for VirtIoBlkDev<H, T> {}

impl<H: Hal, T: Transport> VirtIoBlkDev<H, T> {
    /// Creates a new driver instance and initializes the device, or returns
    /// an error if any step fails.
    pub fn try_new(transport: T) -> DevResult<Self> {
        Ok(Self {
            inner: Mutex::new(InnerDev::new(transport).map_err(as_dev_err)?),
        })
    }
}

impl<H: Hal, T: Transport> BaseDriverOps for VirtIoBlkDev<H, T> {
    fn device_name(&self) -> &str {
        "virtio-blk"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl<H: Hal, T: Transport> BlockDriverOps for VirtIoBlkDev<H, T> {
    #[inline]
    fn num_blocks(&self) -> u64 {
        self.inner.lock().capacity()
    }

    #[inline]
    fn block_size(&self) -> usize {
        virtio_drivers::device::blk::SECTOR_SIZE
    }

    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> DevResult {
        self.inner
            .lock()
            .read_blocks(block_id as _, buf)
            .map_err(as_dev_err)
    }

    fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult {
        self.inner
            .lock()
            .write_blocks(block_id as _, buf)
            .map_err(as_dev_err)
    }

    fn flush(&self) -> DevResult {
        Ok(())
    }
}
