//! SD/MMC platform probe and optional interrupt-backed command wait.

use crate::{AxDeviceEnum, drivers::DriverProbe};

pub struct SdMmcDriver;

#[cfg(feature = "sdmmc-irq")]
mod interrupt {
    use core::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };

    const IRQ: usize = axconfig::devices::SDMMC_IRQ as usize;

    pub(super) struct SdMmcInterrupt;

    pub(super) static INTERRUPT: SdMmcInterrupt = SdMmcInterrupt;
    static PENDING: AtomicBool = AtomicBool::new(false);
    static WAIT_QUEUE: axtask::WaitQueue = axtask::WaitQueue::new();

    fn base() -> usize {
        axhal::mem::phys_to_virt(axconfig::devices::SDMMC_PADDR.into()).into()
    }

    impl axdriver_block::sdmmc::InterruptWait for SdMmcInterrupt {
        fn prepare(&self) {
            PENDING.store(false, Ordering::Release);
        }

        fn wait(&self, timeout: Duration) -> bool {
            !WAIT_QUEUE.wait_timeout_until(timeout, || PENDING.load(Ordering::Acquire))
        }

        fn rearm(&self) {
            PENDING.store(false, Ordering::Release);
            unsafe {
                axdriver_block::sdmmc::set_interrupt_enabled(base(), true);
            }
        }

        fn current_ms(&self) -> u64 {
            axhal::time::monotonic_time_nanos() / 1_000_000
        }
    }

    fn handle() {
        // Deassert the level-triggered output before completing the PLIC
        // claim. The request task acknowledges RINTSTS before rearming it.
        unsafe {
            axdriver_block::sdmmc::set_interrupt_enabled(base(), false);
        }
        PENDING.store(true, Ordering::Release);
        WAIT_QUEUE.notify_all(false);
        axtask::request_reschedule();
    }

    pub(super) fn register() -> bool {
        if axhal::irq::register(IRQ, handle) {
            true
        } else {
            warn!("failed to register SD/MMC IRQ {IRQ}");
            false
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
                axdriver_block::sdmmc::SdMmcDriver::new_with_interrupt(
                    base,
                    &interrupt::INTERRUPT,
                )
            }
            #[cfg(not(feature = "sdmmc-irq"))]
            {
                axdriver_block::sdmmc::SdMmcDriver::new(base)
            }
        };
        Some(AxDeviceEnum::from_block(sdmmc))
    }
}
