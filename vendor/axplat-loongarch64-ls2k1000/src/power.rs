use axplat::power::PowerIf;

use crate::fdt::{SysconAction, uncached_addr};

struct PowerIfImpl;

fn apply(action: SysconAction, operation: &str) -> ! {
    let register = (uncached_addr(action.paddr) + action.offset) as *mut u32;
    info!(
        "LS2K1000 {operation} through DT syscon: paddr={:#x} offset={:#x}",
        action.paddr, action.offset
    );
    unsafe {
        let old = register.read_volatile();
        register.write_volatile((old & !action.mask) | (action.value & action.mask));
        core::arch::asm!("dbar 0", options(nostack, preserves_flags));
    }
    loop {
        axcpu::asm::halt();
    }
}

fn safe_halt(reason: &str) -> ! {
    warn!("LS2K1000 {reason} has no usable DT syscon action; halting safely");
    loop {
        axcpu::asm::halt();
    }
}

#[impl_plat_interface]
impl PowerIf for PowerIfImpl {
    #[cfg(feature = "smp")]
    fn cpu_boot(cpu_id: usize, stack_top_paddr: usize) {
        crate::mp::start_secondary_cpu(cpu_id, stack_top_paddr);
    }

    fn system_reset() -> ! {
        match crate::fdt::resources().reboot {
            Some(action) => apply(action, "reboot"),
            None => safe_halt("reboot"),
        }
    }

    fn system_off() -> ! {
        match crate::fdt::resources().poweroff {
            Some(action) => apply(action, "poweroff"),
            None => safe_halt("poweroff"),
        }
    }
}
