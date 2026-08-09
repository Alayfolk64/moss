//! SD/MMC driver based on SDIO.

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use simple_sdmmc::SdMmc;
pub use simple_sdmmc::InterruptWait;
pub use simple_sdmmc::set_interrupt_enabled;
use spin::Mutex;

use crate::BlockDriverOps;

/// A SD/MMC driver.
pub struct SdMmcDriver(Mutex<SdMmc>);

impl SdMmcDriver {
    /// Creates a new [`SdMmcDriver`] from the given base address.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `base` is a valid pointer to the SD/MMC controller's
    /// register block and that no other code is concurrently accessing the same hardware.
    pub unsafe fn new(base: usize) -> Self {
        Self(Mutex::new(SdMmc::new(base)))
    }

    /// Creates a driver that waits for SD/MMC controller interrupts.
    pub unsafe fn new_with_interrupt(
        base: usize,
        interrupt: &'static dyn InterruptWait,
    ) -> Self {
        Self(Mutex::new(unsafe {
            SdMmc::new_with_interrupt(base, interrupt)
        }))
    }
}

impl BaseDriverOps for SdMmcDriver {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn device_name(&self) -> &str {
        "sdmmc"
    }
}

impl BlockDriverOps for SdMmcDriver {
    fn num_blocks(&self) -> u64 {
        self.0.lock().num_blocks()
    }

    fn block_size(&self) -> usize {
        SdMmc::BLOCK_SIZE
    }

    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> DevResult {
        let (blocks, remainder) = buf.as_chunks_mut::<{ SdMmc::BLOCK_SIZE }>();

        if !remainder.is_empty() {
            return Err(DevError::InvalidParam);
        }

        let mut inner = self.0.lock();
        for (i, block) in blocks.iter_mut().enumerate() {
            inner.read_block(block_id as u32 + i as u32, block);
        }

        Ok(())
    }

    fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult {
        let (blocks, remainder) = buf.as_chunks::<{ SdMmc::BLOCK_SIZE }>();

        if !remainder.is_empty() {
            return Err(DevError::InvalidParam);
        }

        let mut inner = self.0.lock();
        for (i, block) in blocks.iter().enumerate() {
            inner.write_block(block_id as u32 + i as u32, block);
        }

        Ok(())
    }

    fn flush(&self) -> DevResult {
        Ok(())
    }
}
