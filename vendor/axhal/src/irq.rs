//! Interrupt management.

use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "ipi")]
pub use axconfig::devices::IPI_IRQ;
use axcpu::trap::{IRQ, register_trap_handler};
pub use axplat::irq::{handle, register, set_enable, unregister};

/// A logical-CPU target for an inter-processor interrupt.
#[cfg(feature = "ipi")]
pub enum IpiTarget {
    /// Send to the current logical CPU.
    Current { cpu_id: usize },
    /// Send to one other logical CPU.
    Other { cpu_id: usize },
    /// Send to every online logical CPU except the current one.
    AllExceptCurrent { cpu_id: usize, cpu_num: usize },
}

/// Sends an IPI after translating logical CPU IDs at the HAL boundary.
#[cfg(feature = "ipi")]
pub fn send_ipi(irq_num: usize, target: IpiTarget) {
    #[cfg(target_arch = "riscv64")]
    match target {
        IpiTarget::Current { cpu_id } => axplat::irq::send_ipi(
            irq_num,
            axplat::irq::IpiTarget::Current {
                cpu_id: crate::cpu_hardware_id(cpu_id),
            },
        ),
        IpiTarget::Other { cpu_id } => axplat::irq::send_ipi(
            irq_num,
            axplat::irq::IpiTarget::Other {
                cpu_id: crate::cpu_hardware_id(cpu_id),
            },
        ),
        IpiTarget::AllExceptCurrent { cpu_id, cpu_num } => {
            for target_cpu in 0..cpu_num {
                if target_cpu != cpu_id {
                    axplat::irq::send_ipi(
                        irq_num,
                        axplat::irq::IpiTarget::Other {
                            cpu_id: crate::cpu_hardware_id(target_cpu),
                        },
                    );
                }
            }
        }
    }

    #[cfg(not(target_arch = "riscv64"))]
    axplat::irq::send_ipi(
        irq_num,
        match target {
            IpiTarget::Current { cpu_id } => axplat::irq::IpiTarget::Current { cpu_id },
            IpiTarget::Other { cpu_id } => axplat::irq::IpiTarget::Other { cpu_id },
            IpiTarget::AllExceptCurrent { cpu_id, cpu_num } => {
                axplat::irq::IpiTarget::AllExceptCurrent { cpu_id, cpu_num }
            }
        },
    );
}

static IRQ_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Register a hook function called after an IRQ is handled.
///
/// This function can be called only once; subsequent calls will return false.
///
/// TODO: design a better api!
pub fn register_irq_hook(hook: fn(usize)) -> bool {
    IRQ_HOOK
        .compare_exchange(
            0,
            hook as *const () as usize,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

/// IRQ handler.
///
/// # Warn
///
/// Make sure called in an interrupt context or hypervisor VM exit handler.
#[register_trap_handler(IRQ)]
pub fn irq_handler(vector: usize) -> bool {
    let guard = kernel_guard::NoPreempt::new();

    #[cfg(any(feature = "defplat", feature = "myplat"))]
    if let Some(irq) = handle(vector) {
        let hook = IRQ_HOOK.load(Ordering::SeqCst);
        if hook != 0 {
            let hook = unsafe { core::mem::transmute::<usize, fn(usize)>(hook) };
            hook(irq);
        }
    }
    #[cfg(not(any(feature = "defplat", feature = "myplat")))]
    {
        handle(vector);
        let hook = IRQ_HOOK.load(Ordering::SeqCst);
        if hook != 0 {
            let hook = unsafe { core::mem::transmute::<usize, fn(usize)>(hook) };
            hook(vector);
        }
    }

    drop(guard); // rescheduling may occur when preemption is re-enabled.
    true
}
