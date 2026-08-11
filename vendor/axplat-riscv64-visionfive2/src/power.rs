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

    fn system_off() -> ! {
        info!("Shutting down...");
        sbi_rt::system_reset(sbi_rt::Shutdown, sbi_rt::NoReason);
        warn!("SBI system_reset returned unexpectedly");
        loop {
            axcpu::asm::halt();
        }
    }
}
