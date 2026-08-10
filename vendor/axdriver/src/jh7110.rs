//! VisionFive 2 GMAC1 platform glue.

use axdriver_net::dwmac_jh7110::{DwmacJh7110, Jh7110Config, Jh7110Hal};

use crate::{AxDeviceEnum, drivers::DriverProbe};

const CACHE_LINE_SIZE: usize = 64;
const CCACHE_FLUSH64_OFFSET: usize = 0x200;
const CLOCK_ENABLE: u32 = 1 << 31;
const CLOCK_REGISTER_STRIDE: usize = 4;
const GMAC1_CLOCKS: [usize; 5] = [97, 98, 102, 105, 107];
const SYS_RESET_ASSERT_OFFSET: usize = 0x2f8;
const SYS_RESET_STATUS_OFFSET: usize = 0x308;
const GMAC1_AXI_RESET: usize = 66;
const GMAC1_AHB_RESET: usize = 67;
const RESET_TIMEOUT: usize = 100_000;

pub struct GmacHalImpl;

impl GmacHalImpl {
    fn cache_flush(virtual_address: usize, size: usize) {
        if size == 0 {
            return;
        }
        let start = axhal::mem::virt_to_phys(virtual_address.into()).as_usize();
        let Some(end) = start.checked_add(size) else {
            return;
        };
        let mut line = start & !(CACHE_LINE_SIZE - 1);
        let flush = axhal::mem::phys_to_virt(
            (axconfig::devices::CCACHE_PADDR as usize + CCACHE_FLUSH64_OFFSET).into(),
        )
        .as_usize() as *mut u64;
        Self::dma_barrier();
        while line < end {
            unsafe { flush.write_volatile(line as u64) };
            line += CACHE_LINE_SIZE;
        }
        Self::dma_barrier();
    }
}

fn read_register(base: usize, offset: usize) -> u32 {
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

fn write_register(base: usize, offset: usize, value: u32) {
    unsafe { ((base + offset) as *mut u32).write_volatile(value) }
}

fn enable_gmac1_clock_reset() -> bool {
    let base = axhal::mem::phys_to_virt(axconfig::devices::SYSCRG_PADDR.into()).as_usize();

    // U-Boot disables the clocks that it enabled for TFTP before handing the
    // controller to the kernel. Clock registers are indexed by clock ID.
    for clock in GMAC1_CLOCKS {
        let offset = clock * CLOCK_REGISTER_STRIDE;
        write_register(base, offset, read_register(base, offset) | CLOCK_ENABLE);
    }
    GmacHalImpl::dma_barrier();

    // Reset IDs 66 and 67 share the third SYSCRG reset word. Status bits are
    // high after the corresponding reset has been deasserted.
    let reset_word = GMAC1_AXI_RESET / 32;
    let reset_mask = (1 << (GMAC1_AXI_RESET % 32)) | (1 << (GMAC1_AHB_RESET % 32));
    let assert_offset = SYS_RESET_ASSERT_OFFSET + reset_word * size_of::<u32>();
    let status_offset = SYS_RESET_STATUS_OFFSET + reset_word * size_of::<u32>();
    write_register(
        base,
        assert_offset,
        read_register(base, assert_offset) & !reset_mask,
    );
    GmacHalImpl::dma_barrier();

    for _ in 0..RESET_TIMEOUT {
        if read_register(base, status_offset) & reset_mask == reset_mask {
            return true;
        }
        core::hint::spin_loop();
    }
    warn!("JH7110 GMAC1 reset deassertion timed out");
    false
}

impl Jh7110Hal for GmacHalImpl {
    fn virt_to_phys(virtual_address: usize) -> usize {
        axhal::mem::virt_to_phys(virtual_address.into()).as_usize()
    }

    fn current_ms() -> u64 {
        axhal::time::monotonic_time_nanos() / 1_000_000
    }

    fn dma_clean(virtual_address: usize, size: usize) {
        Self::cache_flush(virtual_address, size);
    }

    fn dma_invalidate(virtual_address: usize, size: usize) {
        // JH7110 exposes one L2 clean-and-invalidate command for both DMA
        // ownership directions.
        Self::cache_flush(virtual_address, size);
    }

    fn dma_barrier() {
        unsafe { core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags)) };
    }
}

pub type Jh7110GmacDevice = DwmacJh7110<GmacHalImpl>;

fn firmware_mac_address() -> Option<[u8; 6]> {
    let fdt = axhal::dtb::get_fdt()?;
    let path = fdt.find_aliase("ethernet1")?;
    let node = fdt.find_nodes(path).next()?;
    node.find_property("local-mac-address")
        .or_else(|| node.find_property("mac-address"))
        .and_then(|property| {
            property
                .raw_value()
                .get(..6)
                .and_then(|mac| mac.try_into().ok())
        })
}

pub struct Jh7110GmacDriver;

impl DriverProbe for Jh7110GmacDriver {
    fn probe_global() -> Option<AxDeviceEnum> {
        if !enable_gmac1_clock_reset() {
            return None;
        }

        // Select RGMII in the JH7110 system syscon. U-Boot has already
        // configured and negotiated the external PHY.
        let syscon = axhal::mem::phys_to_virt(axconfig::devices::SYSCON_PADDR.into()).as_usize();
        let mode = unsafe { (syscon as *mut u8).add(0x90).cast::<u32>() };
        let current = unsafe { mode.read_volatile() };
        unsafe { mode.write_volatile((current & !(0x7 << 2)) | (1 << 2)) };

        let base = axhal::mem::phys_to_virt(axconfig::devices::GMAC1_PADDR.into()).as_usize();
        let irq = axconfig::devices::GMAC1_IRQ as usize;
        let mut device = unsafe {
            Jh7110GmacDevice::try_new(
                base,
                Jh7110Config {
                    mac_address: firmware_mac_address(),
                    phy_address: 0,
                    irq: Some(irq),
                },
            )
        }
        .ok()?;

        if axhal::irq::register(irq, axdriver_net::dwmac_jh7110::handle_interrupt) {
            device.enable_interrupt_mode();
        } else {
            warn!("JH7110 GMAC1 IRQ {irq} registration failed; using polling mode");
        }
        Some(AxDeviceEnum::from_net(device))
    }
}
