use core::sync::atomic::{AtomicPtr, Ordering};

use axplat::irq::{IpiTarget, IrqHandler, IrqIf};
use riscv::register::{sie, sip};
use sbi_rt::HartMask;

const INTC_IRQ_BASE: usize = 1 << (usize::BITS - 1);
const S_SOFT: usize = INTC_IRQ_BASE + 1;
const S_TIMER: usize = INTC_IRQ_BASE + 5;
const S_EXT: usize = INTC_IRQ_BASE + 9;

static TIMER_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static IPI_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

pub(super) fn init_percpu() {
    // SD/MMC and UART are polling drivers. Enable only the CPU-local sources
    // needed by scheduling; JH7110 PLIC support is intentionally not claimed.
    unsafe {
        sip::clear_ssoft();
        sie::set_ssoft();
        sie::set_stimer();
        sie::clear_sext();
    }
}

fn replace_handler(slot: &AtomicPtr<()>) -> Option<IrqHandler> {
    let handler = slot.swap(core::ptr::null_mut(), Ordering::AcqRel);
    if handler.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler) })
    }
}

fn call_handler(slot: &AtomicPtr<()>) {
    let handler = slot.load(Ordering::Acquire);
    if !handler.is_null() {
        unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler)() };
    }
}

fn send_ipi_to_logical_cpu(cpu_id: usize) {
    // The JH7110's hart 0 has no S-mode. Moss logical CPU N maps to physical
    // hart N+1; the initial single-core target therefore sends to hart 1.
    let result = sbi_rt::send_ipi(HartMask::from_mask_base(1 << (cpu_id + 1), 0));
    if result.is_err() {
        warn!("send_ipi failed: {result:?}");
    }
}

struct IrqIfImpl;

#[impl_plat_interface]
impl IrqIf for IrqIfImpl {
    fn set_enable(irq: usize, enabled: bool) {
        if irq == S_TIMER {
            unsafe {
                if enabled {
                    sie::set_stimer();
                } else {
                    sie::clear_stimer();
                }
            }
        }
    }

    fn register(irq: usize, handler: IrqHandler) -> bool {
        let slot = match irq {
            S_TIMER => &TIMER_HANDLER,
            S_SOFT => &IPI_HANDLER,
            S_EXT => {
                warn!("VF2 external interrupts are not enabled yet");
                return false;
            }
            _ => {
                warn!("VF2 device IRQ {irq} is not enabled yet");
                return false;
            }
        };
        slot.compare_exchange(
            core::ptr::null_mut(),
            handler as *mut _,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    }

    fn unregister(irq: usize) -> Option<IrqHandler> {
        match irq {
            S_TIMER => replace_handler(&TIMER_HANDLER),
            S_SOFT => replace_handler(&IPI_HANDLER),
            _ => None,
        }
    }

    fn handle(irq: usize) -> Option<usize> {
        match irq {
            S_TIMER => call_handler(&TIMER_HANDLER),
            S_SOFT => {
                unsafe { sip::clear_ssoft() };
                call_handler(&IPI_HANDLER);
            }
            S_EXT => {
                warn!("unexpected VF2 external interrupt");
                return None;
            }
            _ => panic!("unknown VF2 IRQ cause: {irq:#x}"),
        }
        Some(irq)
    }

    fn send_ipi(_irq_num: usize, target: IpiTarget) {
        match target {
            IpiTarget::Current { cpu_id } | IpiTarget::Other { cpu_id } => {
                send_ipi_to_logical_cpu(cpu_id);
            }
            IpiTarget::AllExceptCurrent { cpu_id, cpu_num } => {
                for target_cpu in 0..cpu_num {
                    if target_cpu != cpu_id {
                        send_ipi_to_logical_cpu(target_cpu);
                    }
                }
            }
        }
    }
}
