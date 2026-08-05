use core::sync::atomic::{AtomicUsize, Ordering};

use axconfig::{TASK_STACK_SIZE, plat::CPU_NUM};
use axhal::mem::{VirtAddr, virt_to_phys};

#[unsafe(link_section = ".bss.stack")]
static mut SECONDARY_BOOT_STACK: [[u8; TASK_STACK_SIZE]; CPU_NUM - 1] =
    [[0; TASK_STACK_SIZE]; CPU_NUM - 1];

static ENTERED_CPUS: AtomicUsize = AtomicUsize::new(1);

#[allow(clippy::absurd_extreme_comparisons)]
pub fn start_secondary_cpus(primary_hardware_id: usize) {
    let mut stack_index = 0;
    for logical_id in 0..axhal::cpu_num() {
        let hardware_id = axhal::cpu_hardware_id(logical_id);
        if hardware_id != primary_hardware_id {
            let stack_top = virt_to_phys(VirtAddr::from(unsafe {
                SECONDARY_BOOT_STACK[stack_index].as_ptr_range().end as usize
            }));

            debug!("starting logical CPU {logical_id} (hardware {hardware_id})...");
            axhal::power::cpu_boot(hardware_id, stack_top.as_usize());
            stack_index += 1;

            while ENTERED_CPUS.load(Ordering::Acquire) <= stack_index {
                core::hint::spin_loop();
            }
        }
    }
}

/// The main entry point of the ArceOS runtime for secondary cores.
///
/// It is called from the bootstrapping code in the specific platform crate.
#[axplat::secondary_main]
pub fn rust_main_secondary(hardware_cpu_id: usize) -> ! {
    let cpu_id = axhal::cpu_logical_id(hardware_cpu_id)
        .expect("secondary CPU is missing from the device-tree topology");
    axhal::percpu::init_secondary(cpu_id);
    axhal::init_early_secondary(cpu_id);

    ENTERED_CPUS.fetch_add(1, Ordering::Release);
    info!("Secondary CPU {cpu_id} (hardware {hardware_cpu_id}) started.");

    #[cfg(feature = "paging")]
    axmm::init_memory_management_secondary();

    axhal::init_later_secondary(cpu_id);

    #[cfg(feature = "multitask")]
    axtask::init_scheduler_secondary();

    #[cfg(feature = "ipi")]
    axipi::init();

    info!("Secondary CPU {cpu_id:x} init OK.");
    super::INITED_CPUS.fetch_add(1, Ordering::Release);

    while !super::is_init_ok() {
        core::hint::spin_loop();
    }

    #[cfg(feature = "irq")]
    axhal::asm::enable_irqs();

    #[cfg(all(feature = "tls", not(feature = "multitask")))]
    super::init_tls();

    #[cfg(feature = "multitask")]
    axtask::run_idle();
    #[cfg(not(feature = "multitask"))]
    loop {
        axhal::asm::wait_for_irqs();
    }
}
