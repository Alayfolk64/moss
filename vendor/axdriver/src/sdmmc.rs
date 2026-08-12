//! SD/MMC platform probe and optional interrupt-backed command wait.

use crate::{AxDeviceEnum, drivers::DriverProbe};

pub struct SdMmcDriver;

#[cfg(feature = "sdmmc-irq")]
mod interrupt {
    #[cfg(feature = "sdmmc-idmac")]
    use core::cell::UnsafeCell;
    use core::time::Duration;

    const IRQ: usize = axconfig::devices::SDMMC_IRQ as usize;

    pub(super) struct SdMmcInterrupt;

    pub(super) static INTERRUPT: SdMmcInterrupt = SdMmcInterrupt;
    static MAILBOX: axdriver_block::sdmmc::InterruptMailbox =
        axdriver_block::sdmmc::InterruptMailbox::new();
    static WAIT_QUEUE: axtask::WaitQueue = axtask::WaitQueue::new();

    fn base() -> usize {
        axhal::mem::phys_to_virt(axconfig::devices::SDMMC_PADDR.into()).into()
    }

    impl axdriver_block::sdmmc::InterruptWait for SdMmcInterrupt {
        fn prepare(&self) {
            MAILBOX.begin();
        }

        fn wait(&self, timeout: Duration) -> bool {
            !WAIT_QUEUE.wait_timeout_until(timeout, || MAILBOX.pending())
        }

        fn take_status(&self) -> u32 {
            MAILBOX.take()
        }

        fn finish(&self) {
            MAILBOX.finish();
        }

        fn rearm(&self) {
            unsafe {
                axdriver_block::sdmmc::set_interrupt_enabled(base(), true);
            }
        }

        fn current_ms(&self) -> u64 {
            axhal::time::monotonic_time_nanos() / 1_000_000
        }
    }

    fn handle() {
        let request_generation = MAILBOX.generation();
        let events = unsafe { axdriver_block::sdmmc::acknowledge_interrupt(base()) };
        MAILBOX.cache_if_current(request_generation, events);
        if request_generation != 0 && events != 0 {
            WAIT_QUEUE.notify_all(false);
            axtask::request_reschedule();
        }
    }

    pub(super) fn register() -> bool {
        if axhal::irq::register(IRQ, handle) {
            true
        } else {
            warn!("failed to register SD/MMC IRQ {IRQ}");
            false
        }
    }

    #[cfg(feature = "sdmmc-idmac")]
    const DESCRIPTOR_BYTES: usize = 4096;
    #[cfg(feature = "sdmmc-idmac")]
    const DESCRIPTOR_COUNT: usize = 16;
    #[cfg(feature = "sdmmc-idmac")]
    const BUFFER_BYTES: usize = 64 * 1024;

    #[cfg(feature = "sdmmc-idmac")]
    #[repr(C, align(4096))]
    struct DmaMemory {
        descriptors: [u8; DESCRIPTOR_BYTES],
        buffer: [u8; BUFFER_BYTES],
    }

    #[cfg(feature = "sdmmc-idmac")]
    struct SharedDmaMemory(UnsafeCell<DmaMemory>);

    #[cfg(feature = "sdmmc-idmac")]
    // SAFETY: the block driver serializes all access, and the IRQ handler never
    // touches descriptor or bounce-buffer memory.
    unsafe impl Sync for SharedDmaMemory {}

    #[cfg(feature = "sdmmc-idmac")]
    #[unsafe(link_section = ".dma_uncached")]
    static DMA_MEMORY: SharedDmaMemory = SharedDmaMemory(UnsafeCell::new(DmaMemory {
        descriptors: [0; DESCRIPTOR_BYTES],
        buffer: [0; BUFFER_BYTES],
    }));

    #[cfg(feature = "sdmmc-idmac")]
    fn virt_to_phys(address: usize) -> usize {
        axhal::mem::virt_to_phys(address.into()).as_usize()
    }

    #[cfg(feature = "sdmmc-idmac")]
    fn dma_barrier() {
        unsafe { core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags)) };
    }

    #[cfg(feature = "sdmmc-idmac")]
    pub(super) fn dma_config() -> axdriver_block::sdmmc::DmaConfig {
        let memory = DMA_MEMORY.0.get() as usize;
        // SAFETY: DMA_MEMORY is static, exclusively controller-owned,
        // 4-KiB-aligned, and placed in the platform's uncached linker section.
        unsafe {
            axdriver_block::sdmmc::DmaConfig::new(
                memory,
                DESCRIPTOR_COUNT,
                memory + DESCRIPTOR_BYTES,
                BUFFER_BYTES,
                virt_to_phys,
                dma_barrier,
            )
        }
    }
}

impl DriverProbe for SdMmcDriver {
    fn probe_global() -> Option<AxDeviceEnum> {
        #[cfg(feature = "sdmmc-irq")]
        if !interrupt::register() {
            return None;
        }

        let base = axhal::mem::phys_to_virt(axconfig::devices::SDMMC_PADDR.into()).into();
        let sdmmc = unsafe {
            #[cfg(feature = "sdmmc-irq")]
            {
                #[cfg(feature = "sdmmc-idmac")]
                {
                    axdriver_block::sdmmc::SdMmcDriver::new_with_interrupt_and_dma(
                        base,
                        &interrupt::INTERRUPT,
                        interrupt::dma_config(),
                    )
                }
                #[cfg(not(feature = "sdmmc-idmac"))]
                {
                    axdriver_block::sdmmc::SdMmcDriver::new_with_interrupt(
                        base,
                        &interrupt::INTERRUPT,
                    )
                }
            }
            #[cfg(not(feature = "sdmmc-irq"))]
            {
                axdriver_block::sdmmc::SdMmcDriver::new(base)
            }
        };
        Some(AxDeviceEnum::from_block(sdmmc))
    }
}
