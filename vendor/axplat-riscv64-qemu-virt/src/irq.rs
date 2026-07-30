use core::{
    num::NonZeroU32,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, AtomicU64, Ordering},
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

/// `Interrupt` bit in `scause`
pub(super) const INTC_IRQ_BASE: usize = 1 << (usize::BITS - 1);

/// Supervisor software interrupt in `scause`
#[allow(unused)]
pub(super) const S_SOFT: usize = INTC_IRQ_BASE + 1;

/// Supervisor timer interrupt in `scause`
pub(super) const S_TIMER: usize = INTC_IRQ_BASE + 5;

/// Supervisor external interrupt in `scause`
pub(super) const S_EXT: usize = INTC_IRQ_BASE + 9;

static TIMER_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

static IPI_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SBI_IPI_CALLS: AtomicU64 = AtomicU64::new(0);
static SBI_IPI_UNICASTS: AtomicU64 = AtomicU64::new(0);
static SBI_IPI_BROADCASTS: AtomicU64 = AtomicU64::new(0);
static SBI_IPI_BROADCAST_HARTS: AtomicU64 = AtomicU64::new(0);
static RECEIVED_IPIS: AtomicU64 = AtomicU64::new(0);

/// The maximum number of IRQs.
pub const MAX_IRQ_COUNT: usize = 1024;

static IRQ_HANDLER_TABLE: HandlerTable<MAX_IRQ_COUNT> = HandlerTable::new();

static PLIC: SpinNoIrq<Plic> = SpinNoIrq::new(unsafe {
    Plic::new(NonNull::new((PHYS_VIRT_OFFSET + PLIC_PADDR) as *mut _).unwrap())
});

fn this_context() -> usize {
    let hart_id = this_cpu_id();
    hart_id * 2 + 1 // supervisor context
}

pub(super) fn init_percpu() {
    // enable soft interrupts, timer interrupts, and external interrupts
    unsafe {
        sie::set_ssoft();
        sie::set_stimer();
        sie::set_sext();
    }
    PLIC.lock().init_by_context(this_context());
}

pub(crate) fn log_ipi_stats() {
    let calls = SBI_IPI_CALLS.load(Ordering::Relaxed);
    let unicasts = SBI_IPI_UNICASTS.load(Ordering::Relaxed);
    let broadcasts = SBI_IPI_BROADCASTS.load(Ordering::Relaxed);
    let broadcast_harts = SBI_IPI_BROADCAST_HARTS.load(Ordering::Relaxed);
    let received = RECEIVED_IPIS.load(Ordering::Relaxed);
    let old_calls = unicasts.saturating_add(broadcast_harts);
    warn!(
        "[ipi] sbi_calls={calls} unicasts={unicasts} broadcasts={broadcasts} broadcast_harts={broadcast_harts} received={received} old_per_hart_calls={old_calls} saved_calls={}",
        old_calls.saturating_sub(calls),
    );
}

macro_rules! with_cause {
    ($cause: expr, @S_TIMER => $timer_op: expr, @S_SOFT => $ipi_op: expr, @S_EXT => $ext_op: expr, @EX_IRQ => $plic_op: expr $(,)?) => {
        match $cause {
            S_TIMER => $timer_op,
            S_SOFT => $ipi_op,
            S_EXT => $ext_op,
            other => {
                if other & INTC_IRQ_BASE == 0 {
                    // Device-side interrupts read from PLIC
                    $plic_op
                } else {
                    // Other CPU-side interrupts
                    panic!("Unknown IRQ cause: {other}");
                }
            }
        }
    };
}

struct IrqIfImpl;

#[impl_plat_interface]
impl IrqIf for IrqIfImpl {
    /// Enables or disables the given IRQ.
    fn set_enable(irq: usize, enabled: bool) {
        with_cause!(
            irq,
            @S_TIMER => {
                unsafe {
                    if enabled {
                        sie::set_stimer();
                    } else {
                        sie::clear_stimer();
                    }
                }
            },
            @S_SOFT => {},
            @S_EXT => {},
            @EX_IRQ => {
                let Some(irq) = NonZeroU32::new(irq as _) else {
                    return;
                };
                trace!("PLIC set enable: {irq} {enabled}");
                let mut plic = PLIC.lock();
                if enabled {
                    plic.set_priority(irq, 6);
                    plic.enable(irq, this_context());
                } else {
                    plic.disable(irq, this_context());
                }
            }
        );
    }

    /// Registers an IRQ handler for the given IRQ.
    ///
    /// It also enables the IRQ if the registration succeeds. It returns `false` if
    /// the registration failed.
    ///
    /// The `irq` parameter has the following semantics
    /// 1. If its highest bit is 1, it means it is an interrupt on the CPU side. Its
    /// value comes from `scause`, where [`S_SOFT`] represents software interrupt
    /// and [`S_TIMER`] represents timer interrupt. If its value is [`S_EXT`], it
    /// means it is an external interrupt, and the real IRQ number needs to
    /// be obtained from PLIC.
    /// 2. If its highest bit is 0, it means it is an interrupt on the device side,
    /// and its value is equal to the IRQ number provided by PLIC.
    fn register(irq: usize, handler: IrqHandler) -> bool {
        with_cause!(
            irq,
            @S_TIMER => TIMER_HANDLER.compare_exchange(core::ptr::null_mut(), handler as *mut _, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            @S_SOFT => IPI_HANDLER.compare_exchange(core::ptr::null_mut(), handler as *mut _, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            @S_EXT => {
                warn!("External IRQ should be got from PLIC, not scause");
                false
            },
            @EX_IRQ => {
                if IRQ_HANDLER_TABLE.register_handler(irq, handler) {
                    Self::set_enable(irq, true);
                    true
                } else {
                    warn!("register handler for External IRQ {irq} failed");
                    false
                }
            }
        )
    }

    /// Unregisters the IRQ handler for the given IRQ.
    ///
    /// It also disables the IRQ if the unregistration succeeds. It returns the
    /// existing handler if it is registered, `None` otherwise.
    fn unregister(irq: usize) -> Option<IrqHandler> {
        with_cause!(
            irq,
            @S_TIMER => {
                let handler = TIMER_HANDLER.swap(core::ptr::null_mut(), Ordering::AcqRel);
                if !handler.is_null() {
                    Some(unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler) })
                } else {
                    None
                }
            },
            @S_SOFT => {
                let handler = IPI_HANDLER.swap(core::ptr::null_mut(), Ordering::AcqRel);
                if !handler.is_null() {
                    Some(unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler) })
                } else {
                    None
                }
            },
            @S_EXT => {
                warn!("External IRQ should be got from PLIC, not scause");
                None
            },
            @EX_IRQ => IRQ_HANDLER_TABLE.unregister_handler(irq).inspect(|_| Self::set_enable(irq, false))
        )
    }

    /// Handles the IRQ.
    ///
    /// It is called by the common interrupt handler. It should look up in the
    /// IRQ handler table and calls the corresponding handler. If necessary, it
    /// also acknowledges the interrupt controller after handling.
    fn handle(irq: usize) -> Option<usize> {
        with_cause!(
            irq,
            @S_TIMER => {
                trace!("IRQ: timer");
                let handler = TIMER_HANDLER.load(Ordering::Acquire);
                if !handler.is_null() {
                    // SAFETY: The handler is guaranteed to be a valid function pointer.
                    unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler)() };
                }
                Some(irq)
            },
            @S_SOFT => {
                trace!("IRQ: IPI");
                RECEIVED_IPIS.fetch_add(1, Ordering::Relaxed);
                // SBI raises an IPI by setting SSIP. Acknowledge it before
                // running callbacks so the hart does not immediately trap
                // again after returning from this handler.
                unsafe { sip::clear_ssoft() };
                let handler = IPI_HANDLER.load(Ordering::Acquire);
                if !handler.is_null() {
                    // SAFETY: The handler is guaranteed to be a valid function pointer.
                    unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler)() };
                }
                Some(irq)
            },
            @S_EXT => {
                let mut plic = PLIC.lock();
                let Some(irq) = plic.claim(this_context()) else {
                    debug!("Spurious external IRQ");
                    return None;
                };
                trace!("IRQ: external {irq}");
                IRQ_HANDLER_TABLE.handle(irq.get() as usize);
                plic.complete(this_context(), irq);
                Some(irq.get() as usize)
            },
            @EX_IRQ => {
                unreachable!("Device-side IRQs should be handled by triggering the External Interrupt.");
            }
        )
    }

    /// Sends an inter-processor interrupt (IPI) to the specified target CPU or all CPUs.
    fn send_ipi(_irq_num: usize, target: IpiTarget) {
        match target {
            IpiTarget::Current { cpu_id } => {
                SBI_IPI_CALLS.fetch_add(1, Ordering::Relaxed);
                SBI_IPI_UNICASTS.fetch_add(1, Ordering::Relaxed);
                let res = sbi_rt::send_ipi(HartMask::from_mask_base(1, cpu_id));
                if res.is_err() {
                    warn!("send_ipi failed: {res:?}");
                }
            }
            IpiTarget::Other { cpu_id } => {
                SBI_IPI_CALLS.fetch_add(1, Ordering::Relaxed);
                SBI_IPI_UNICASTS.fetch_add(1, Ordering::Relaxed);
                let res = sbi_rt::send_ipi(HartMask::from_mask_base(1, cpu_id));
                if res.is_err() {
                    warn!("send_ipi failed: {res:?}");
                }
            }
            IpiTarget::AllExceptCurrent { cpu_id, cpu_num } => {
                // SBI accepts a hart bitmap. Send one ecall per machine-word
                // chunk instead of entering M-mode once for every target CPU.
                SBI_IPI_BROADCASTS.fetch_add(1, Ordering::Relaxed);
                SBI_IPI_BROADCAST_HARTS
                    .fetch_add(cpu_num.saturating_sub(1) as u64, Ordering::Relaxed);
                let mask_width = usize::BITS as usize;
                for base in (0..cpu_num).step_by(mask_width) {
                    let count = (cpu_num - base).min(mask_width);
                    let mut target_mask = if count == mask_width {
                        usize::MAX
                    } else {
                        (1usize << count) - 1
                    };
                    if (base..base + count).contains(&cpu_id) {
                        target_mask &= !(1usize << (cpu_id - base));
                    }
                    if target_mask == 0 {
                        continue;
                    }
                    SBI_IPI_CALLS.fetch_add(1, Ordering::Relaxed);
                    let res = sbi_rt::send_ipi(HartMask::from_mask_base(target_mask, base));
                    if res.is_err() {
                        warn!("send_ipi_all_others failed: {res:?}");
                    }
                }
            }
        }
    }
}
