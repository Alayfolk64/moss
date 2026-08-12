use axplat::init::InitIf;

struct InitIfImpl;

#[impl_plat_interface]
impl InitIf for InitIfImpl {
    fn init_early(_cpu_id: usize, arg: usize) {
        axcpu::init::init_trap();
        crate::fdt::init(arg);
        // The serial/TFTP runner arms the watchdog before entering MOSS.
        // Production reloads it as soon as the DT resource is known; the
        // interactive image explicitly disarms the inherited firmware state.
        #[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
        if !crate::watchdog::start(crate::watchdog::BOOT_TIMEOUT_SECS) {
            warn!("LS2K1000 early watchdog is unavailable; early hang recovery is disabled");
        }
        #[cfg(feature = "shell-mode")]
        crate::watchdog::stop();
        crate::console::init_early();
        crate::time::init_early();
        crate::node_clock::boost_to_900mhz();

        // This marker is intentionally emitted before the allocator, drivers,
        // filesystem and userspace are initialized.  It is the first hardware
        // acceptance criterion for the LS2K1000LA port.
        axplat::console_println!("MOSS LS2K1000LA early console ready");
    }

    #[cfg(feature = "smp")]
    fn init_early_secondary(_cpu_id: usize) {
        axcpu::init::init_trap();
    }

    fn init_later(_cpu_id: usize, _arg: usize) {
        #[cfg(feature = "irq")]
        {
            crate::irq::init();
            crate::console::init_irq();
        }
        crate::time::init_percpu();
        #[cfg(all(feature = "smp", feature = "irq"))]
        axplat::irq::set_enable(crate::config::devices::IPI_IRQ, true);
        #[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
        if !crate::watchdog::start(crate::watchdog::BOOT_TIMEOUT_SECS) {
            warn!("LS2K1000 watchdog is unavailable; automatic hang recovery is disabled");
        }
    }

    #[cfg(feature = "smp")]
    fn init_later_secondary(_cpu_id: usize) {
        crate::time::init_percpu();
        #[cfg(feature = "irq")]
        axplat::irq::set_enable(crate::config::devices::IPI_IRQ, true);
    }
}
