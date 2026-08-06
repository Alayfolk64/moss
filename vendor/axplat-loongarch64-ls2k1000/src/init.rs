use axplat::init::InitIf;

struct InitIfImpl;

#[impl_plat_interface]
impl InitIf for InitIfImpl {
    fn init_early(_cpu_id: usize, arg: usize) {
        axcpu::init::init_trap();
        crate::fdt::init(arg);
        crate::console::init_early();
        crate::time::init_early();

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
        crate::irq::init();
        crate::time::init_percpu();
        #[cfg(all(feature = "smp", feature = "irq"))]
        axplat::irq::set_enable(crate::config::devices::IPI_IRQ, true);
    }

    #[cfg(feature = "smp")]
    fn init_later_secondary(_cpu_id: usize) {
        crate::time::init_percpu();
        #[cfg(feature = "irq")]
        axplat::irq::set_enable(crate::config::devices::IPI_IRQ, true);
    }
}
