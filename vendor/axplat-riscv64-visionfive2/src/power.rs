use axplat::power::PowerIf;

struct PowerImpl;

#[impl_plat_interface]
impl PowerIf for PowerImpl {
    #[cfg(feature = "smp")]
    fn cpu_boot(hart_id: usize, stack_top_paddr: usize) {
        use axplat::mem::{va, virt_to_phys};

        assert!(
            sbi_rt::probe_extension(sbi_rt::Hsm).is_available(),
            "VF2 SMP requires the SBI HSM extension"
        );
        let entry = virt_to_phys(va!(crate::boot::_start_secondary as *const () as usize));
        let result = sbi_rt::hart_start(hart_id, entry.as_usize(), stack_top_paddr);
        assert!(
            !result.is_err(),
            "SBI hart_start failed for hart {hart_id}: {result:?}"
        );
    }

    fn system_reset() -> ! {
        info!("Rebooting...");
        #[cfg(feature = "watchdog")]
        if crate::watchdog::start(2) {
            warn!("waiting for VF2 hardware watchdog reset");
            loop {
                axcpu::asm::halt();
            }
        }

        // Keep SBI reset as the fallback when the watchdog is not compiled in
        // or its clock/reset/MMIO setup could not be established.
        let result = sbi_rt::system_reset(sbi_rt::ColdReboot, sbi_rt::NoReason);
        warn!("SBI cold reboot returned unexpectedly: {result:?}");
        Self::system_off()
    }

    fn watchdog_start(timeout_secs: u32) -> bool {
        #[cfg(feature = "watchdog")]
        return crate::watchdog::start(timeout_secs);
        #[cfg(not(feature = "watchdog"))]
        {
            let _ = timeout_secs;
            false
        }
    }

    fn watchdog_ping() {
        #[cfg(feature = "watchdog")]
        crate::watchdog::ping();
    }

    fn watchdog_stop() {
        #[cfg(feature = "watchdog")]
        crate::watchdog::stop();
    }

    fn system_off() -> ! {
        info!("Shutting down...");
        sbi_rt::system_reset(sbi_rt::Shutdown, sbi_rt::NoReason);
        warn!("SBI system_reset returned unexpectedly");
        loop {
            axcpu::asm::halt();
        }
    }
}
