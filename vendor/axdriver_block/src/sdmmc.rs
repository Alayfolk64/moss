//! SD/MMC driver based on SDIO.

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use simple_sdmmc::SdMmc;
pub use simple_sdmmc::{
    DmaConfig, InterruptMailbox, InterruptWait, acknowledge_interrupt, set_interrupt_enabled,
};
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
    pub unsafe fn new_with_interrupt(base: usize, interrupt: &'static dyn InterruptWait) -> Self {
        Self(Mutex::new(unsafe {
            SdMmc::new_with_interrupt(base, interrupt)
        }))
    }

    /// Creates an interrupt-driven driver backed by DWMMC IDMAC memory.
    pub unsafe fn new_with_interrupt_and_dma(
        base: usize,
        interrupt: &'static dyn InterruptWait,
        dma: DmaConfig,
    ) -> Self {
        Self(Mutex::new(unsafe {
            SdMmc::new_with_interrupt_and_dma(base, interrupt, dma)
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

        let end_block = block_id
            .checked_add(blocks.len() as u64)
            .ok_or(DevError::InvalidParam)?;
        let mut inner = self.0.lock();
        if end_block > inner.num_blocks() || end_block > u32::MAX as u64 + 1 {
            return Err(DevError::InvalidParam);
        }
        let max_bytes = inner.max_transfer_blocks() * SdMmc::BLOCK_SIZE;
        let mut offset = 0usize;
        for data in buf.chunks_mut(max_bytes) {
            inner
                .read_blocks(block_id as u32 + (offset / SdMmc::BLOCK_SIZE) as u32, data)
                .map_err(|_| DevError::Io)?;
            offset += data.len();
        }

        Ok(())
    }

    fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult {
        let (blocks, remainder) = buf.as_chunks::<{ SdMmc::BLOCK_SIZE }>();

        if !remainder.is_empty() {
            return Err(DevError::InvalidParam);
        }

        let end_block = block_id
            .checked_add(blocks.len() as u64)
            .ok_or(DevError::InvalidParam)?;
        let mut inner = self.0.lock();
        if end_block > inner.num_blocks() || end_block > u32::MAX as u64 + 1 {
            return Err(DevError::InvalidParam);
        }
        let max_bytes = inner.max_transfer_blocks() * SdMmc::BLOCK_SIZE;
        let mut offset = 0usize;
        for data in buf.chunks(max_bytes) {
            inner
                .write_blocks(block_id as u32 + (offset / SdMmc::BLOCK_SIZE) as u32, data)
                .map_err(|_| DevError::Io)?;
            offset += data.len();
        }

        Ok(())
    }

    fn flush(&self) -> DevResult {
        self.0.lock().flush().map_err(|_| DevError::Io)
    }
}
