use axplat::init::InitIf;

struct InitIfImpl;

#[impl_plat_interface]
impl InitIf for InitIfImpl {
    fn init_early(_cpu_id: usize, _mbi: usize) {
        axcpu::init::init_trap();
        crate::console::init_early();
        crate::time::init_early();
    }

    fn init_later(_cpu_id: usize, _arg: usize) {
        #[cfg(feature = "irq")]
        {
            crate::irq::init_percpu();
            crate::console::init_irq();
        }
        crate::time::init_percpu();
    }
}
