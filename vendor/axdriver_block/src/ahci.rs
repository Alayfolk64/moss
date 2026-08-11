//! AHCI driver.

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use simple_ahci::AhciDriver as SimpleAhciDriver;
pub use simple_ahci::{AhciIrqHandle, Hal as AhciHal};

use crate::BlockDriverOps;

/// AHCI driver based on the `simple_ahci` crate.
pub struct AhciDriver<H: AhciHal>(SimpleAhciDriver<H>);

/// An AHCI controller whose MMIO and DMA structures are initialized while
/// device interrupts remain disabled.
pub struct PreparedAhciDriver<H: AhciHal>(simple_ahci::PreparedAhci<H>);

impl<H: AhciHal> AhciDriver<H> {
    pub unsafe fn prepare(base: usize) -> Option<PreparedAhciDriver<H>> {
        unsafe { SimpleAhciDriver::<H>::prepare(base) }.map(PreparedAhciDriver)
    }

    pub unsafe fn prepare_with_port_map(
        base: usize,
        platform_port_map: Option<u32>,
    ) -> Option<PreparedAhciDriver<H>> {
        unsafe { SimpleAhciDriver::<H>::prepare_with_port_map(base, platform_port_map) }
            .map(PreparedAhciDriver)
    }

    /// Try to construct a new AHCI driver from the given MMIO base address.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - `base` is a valid virtual address pointing to the AHCI controller's MMIO register block.
    /// - The memory region starting at `base` is properly mapped and accessible.
    /// - No other code is concurrently accessing the same AHCI controller.
    /// - The AHCI controller hardware is present and functional at the given address.
    pub unsafe fn try_new(base: usize) -> Option<Self> {
        unsafe { SimpleAhciDriver::<H>::try_new(base) }.map(AhciDriver)
    }

    pub fn enable_runtime_waits(&self) {
        self.0.enable_runtime_waits();
    }
}

impl<H: AhciHal> PreparedAhciDriver<H> {
    pub fn irq_handle(&self) -> AhciIrqHandle<H> {
        self.0.irq_handle()
    }

    pub fn enable_interrupts(&self) {
        self.0.enable_interrupts();
    }

    pub fn identify(self) -> Option<AhciDriver<H>> {
        self.0.identify().map(AhciDriver)
    }
}

impl<H: AhciHal> BaseDriverOps for AhciDriver<H> {
    fn device_name(&self) -> &str {
        "ahci"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl<H: AhciHal> BlockDriverOps for AhciDriver<H> {
    fn block_size(&self) -> usize {
        self.0.block_size()
    }

    fn num_blocks(&self) -> u64 {
        self.0.capacity()
    }

    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> DevResult {
        if buf.len() % self.block_size() != 0 {
            return Err(DevError::InvalidParam);
        }
        if buf.as_ptr() as usize % 4 != 0 {
            return Err(DevError::InvalidParam);
        }
        if self.0.read(block_id, buf) {
            Ok(())
        } else {
            Err(DevError::Io)
        }
    }

    fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult {
        if buf.len() % self.block_size() != 0 {
            return Err(DevError::InvalidParam);
        }
        if buf.as_ptr() as usize % 4 != 0 {
            return Err(DevError::InvalidParam);
        }
        if self.0.write(block_id, buf) {
            Ok(())
        } else {
            Err(DevError::Io)
        }
    }

    fn flush(&self) -> DevResult {
        self.0.flush().then_some(()).ok_or(DevError::Io)
    }
}
