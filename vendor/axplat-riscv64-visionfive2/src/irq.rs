use core::{
    num::NonZeroU32,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, Ordering},
};

use axplat::{
    irq::{HandlerTable, IpiTarget, IrqHandler, IrqIf},
    percpu::this_cpu_id,
};
use kspin::SpinNoIrq;
use riscv::register::{sie, sip};
use riscv_plic::Plic;
use sbi_rt::HartMask;

use crate::config::{devices::PLIC_PADDR, plat::PHYS_VIRT_OFFSET};

const INTC_IRQ_BASE: usize = 1 << (usize::BITS - 1);
const S_SOFT: usize = INTC_IRQ_BASE + 1;
const S_TIMER: usize = INTC_IRQ_BASE + 5;
const S_EXT: usize = INTC_IRQ_BASE + 9;

const PLIC_SOURCE_COUNT: usize = crate::config::devices::PLIC_NUM_SOURCES;
const PLIC_PRIORITY: u32 = 1;

static TIMER_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static IPI_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static DEVICE_HANDLERS: HandlerTable<{ PLIC_SOURCE_COUNT + 1 }> = HandlerTable::new();

static PLIC: SpinNoIrq<Plic> = SpinNoIrq::new(unsafe {
    Plic::new(NonNull::new((PHYS_VIRT_OFFSET + PLIC_PADDR) as *mut _).unwrap())
});

fn plic_context(cpu_id: usize) -> usize {
    // JH7110 hart 0 has no S-mode context. Starting with physical hart 1,
    // every hart contributes an M-mode context followed by an S-mode context:
    // logical CPU 0 -> physical hart 1 -> PLIC context 2.
    (cpu_id + 1) * 2
}

fn this_context() -> usize {
    plic_context(this_cpu_id())
}

pub(super) fn init_percpu() {
    let context = this_context();
    {
        let mut plic = PLIC.lock();
        plic.init_by_context(context);
        // Do not inherit source enables left in this S-mode context by an
        // earlier payload. Drivers enable their source after registering a
        // handler with the platform IRQ layer.
        for source in 1..=PLIC_SOURCE_COUNT {
            plic.disable(NonZeroU32::new(source as u32).unwrap(), context);
        }
    }

    unsafe {
        sip::clear_ssoft();
        sie::set_ssoft();
        sie::set_stimer();
        sie::set_sext();
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

fn device_irq(irq: usize) -> Option<NonZeroU32> {
    let source = u32::try_from(irq).ok().and_then(NonZeroU32::new)?;
    (irq <= PLIC_SOURCE_COUNT).then_some(source)
}

struct IrqIfImpl;

#[impl_plat_interface]
impl IrqIf for IrqIfImpl {
    fn set_enable(irq: usize, enabled: bool) {
        match irq {
            S_TIMER => unsafe {
                if enabled {
                    sie::set_stimer();
                } else {
                    sie::clear_stimer();
                }
            },
            S_SOFT | S_EXT => {}
            _ => {
                let Some(source) = device_irq(irq) else {
                    warn!("invalid VF2 PLIC source {irq}");
                    return;
                };
                let mut plic = PLIC.lock();
                if enabled {
                    plic.set_priority(source, PLIC_PRIORITY);
                    plic.enable(source, this_context());
                } else {
                    plic.disable(source, this_context());
                }
            }
        }
    }

    fn register(irq: usize, handler: IrqHandler) -> bool {
        match irq {
            S_TIMER => TIMER_HANDLER
                .compare_exchange(
                    core::ptr::null_mut(),
                    handler as *mut _,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok(),
            S_SOFT => IPI_HANDLER
                .compare_exchange(
                    core::ptr::null_mut(),
                    handler as *mut _,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok(),
            S_EXT => {
                warn!("register the PLIC source number, not the S_EXT scause value");
                false
            }
            _ => {
                if device_irq(irq).is_none() || !DEVICE_HANDLERS.register_handler(irq, handler) {
                    warn!("register handler for VF2 PLIC source {irq} failed");
                    return false;
                }
                Self::set_enable(irq, true);
                true
            }
        }
    }

    fn unregister(irq: usize) -> Option<IrqHandler> {
        match irq {
            S_TIMER => replace_handler(&TIMER_HANDLER),
            S_SOFT => replace_handler(&IPI_HANDLER),
            S_EXT => None,
            _ => DEVICE_HANDLERS
                .unregister_handler(irq)
                .inspect(|_| Self::set_enable(irq, false)),
        }
    }

    fn handle(irq: usize) -> Option<usize> {
        match irq {
            S_TIMER => {
                call_handler(&TIMER_HANDLER);
                Some(S_TIMER)
            }
            S_SOFT => {
                unsafe { sip::clear_ssoft() };
                call_handler(&IPI_HANDLER);
                Some(S_SOFT)
            }
            S_EXT => {
                let context = this_context();
                let source = PLIC.lock().claim(context)?;
                let irq = source.get() as usize;

                if !DEVICE_HANDLERS.handle(irq) {
                    PLIC.lock().disable(source, context);
                    warn!("unhandled VF2 PLIC source {irq}; source disabled");
                }

                PLIC.lock().complete(context, source);
                Some(irq)
            }
            _ if irq & INTC_IRQ_BASE == 0 => {
                unreachable!("device IRQs are dispatched through the S_EXT trap")
            }
            _ => panic!("unknown VF2 IRQ cause: {irq:#x}"),
        }
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
