use axplat::irq::{HandlerTable, IpiTarget, IrqHandler, IrqIf};
use loongArch64::{
    iocsr::{iocsr_read_w, iocsr_write_w},
    register::{
        ecfg::{self, LineBasedInterrupt},
        ticlr,
    },
};

use crate::config::devices::{EIOINTC_IRQ, IPI_IRQ, TIMER_IRQ};

// TODO: move these modules to a separate crate
mod eiointc;
mod pch_pic;

/// The maximum number of IRQs.
pub const MAX_IRQ_COUNT: usize = 13;

const IOCSR_IPI_STATUS: usize = 0x1000;
const IOCSR_IPI_ENABLE: usize = 0x1004;
const IOCSR_IPI_CLEAR: usize = 0x100c;
const IOCSR_IPI_SEND: usize = 0x1040;
const IOCSR_IPI_SEND_CPU_SHIFT: u32 = 16;
const IOCSR_IPI_SEND_BLOCKING: u32 = 1 << 31;

static IRQ_HANDLER_TABLE: HandlerTable<MAX_IRQ_COUNT> = HandlerTable::new();

pub(crate) fn init() {
    eiointc::init();
    pch_pic::init();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IrqType {
    Timer,
    Ipi,
    Io,
    Ex(usize),
}

impl IrqType {
    fn new(irq: usize) -> Self {
        match irq {
            TIMER_IRQ => Self::Timer,
            IPI_IRQ => Self::Ipi,
            EIOINTC_IRQ => Self::Io,
            n => Self::Ex(n),
        }
    }

    fn as_usize(&self) -> usize {
        match self {
            IrqType::Timer => TIMER_IRQ,
            IrqType::Ipi => IPI_IRQ,
            IrqType::Io => EIOINTC_IRQ,
            IrqType::Ex(n) => *n,
        }
    }
}

fn send_ipi(cpu_id: usize) {
    let value = IOCSR_IPI_SEND_BLOCKING
        | ((cpu_id as u32) << IOCSR_IPI_SEND_CPU_SHIFT);
    iocsr_write_w(IOCSR_IPI_SEND, value);
}

fn handle_ipi() {
    let status = iocsr_read_w(IOCSR_IPI_STATUS);
    if status == 0 {
        return;
    }

    iocsr_write_w(IOCSR_IPI_CLEAR, status);
    if !IRQ_HANDLER_TABLE.handle(IPI_IRQ) {
        debug!("Unhandled IPI");
    }
}

struct IrqIfImpl;

#[impl_plat_interface]
impl IrqIf for IrqIfImpl {
    /// Enables or disables the given IRQ.
    fn set_enable(irq: usize, enabled: bool) {
        let irq = IrqType::new(irq);

        match irq {
            IrqType::Timer => {
                let old_value = ecfg::read().lie();
                let new_value = match enabled {
                    true => old_value | LineBasedInterrupt::TIMER,
                    false => old_value & !LineBasedInterrupt::TIMER,
                };
                ecfg::set_lie(new_value);
            }
            IrqType::Ipi => {
                iocsr_write_w(IOCSR_IPI_ENABLE, if enabled { u32::MAX } else { 0 });
                let old_value = ecfg::read().lie();
                let new_value = if enabled {
                    old_value | LineBasedInterrupt::IPI
                } else {
                    old_value & !LineBasedInterrupt::IPI
                };
                ecfg::set_lie(new_value);
            }
            IrqType::Io => {}
            IrqType::Ex(irq) => {
                if enabled {
                    eiointc::enable_irq(irq);
                    pch_pic::enable_irq(irq);
                } else {
                    eiointc::disable_irq(irq);
                    pch_pic::disable_irq(irq);
                }
            }
        }
    }

    /// Registers an IRQ handler for the given IRQ.
    fn register(irq: usize, handler: IrqHandler) -> bool {
        if IRQ_HANDLER_TABLE.register_handler(irq, handler) {
            Self::set_enable(irq, true);
            return true;
        }
        warn!("register handler for IRQ {} failed", irq);
        false
    }

    /// Unregisters the IRQ handler for the given IRQ.
    ///
    /// It also disables the IRQ if the unregistration succeeds. It returns the
    /// existing handler if it is registered, `None` otherwise.
    fn unregister(irq: usize) -> Option<IrqHandler> {
        IRQ_HANDLER_TABLE
            .unregister_handler(irq)
            .inspect(|_| Self::set_enable(irq, false))
    }

    /// Handles the IRQ.
    ///
    /// It is called by the common interrupt handler. It should look up in the
    /// IRQ handler table and calls the corresponding handler. If necessary, it
    /// also acknowledges the interrupt controller after handling.
    fn handle(irq: usize) -> Option<usize> {
        let mut irq = IrqType::new(irq);

        if matches!(irq, IrqType::Io) {
            let Some(ex_irq) = eiointc::claim_irq() else {
                debug!("Spurious external IRQ");
                return None;
            };
            irq = IrqType::Ex(ex_irq);
        }

        trace!("IRQ {irq:?}");

        if matches!(irq, IrqType::Ipi) {
            handle_ipi();
            return Some(IPI_IRQ);
        }

        if matches!(irq, IrqType::Timer) {
            // Acknowledge the expired one-shot before the handler arms the next
            // one. Clearing it after reprogramming can drop a timer event if
            // the next deadline expires while the handler is still running.
            ticlr::clear_timer_interrupt();
        }

        if !IRQ_HANDLER_TABLE.handle(irq.as_usize()) {
            debug!("Unhandled IRQ {irq:?}");
        }

        match irq {
            IrqType::Timer => {}
            IrqType::Ipi => unreachable!(),
            IrqType::Io => {}
            IrqType::Ex(irq) => {
                eiointc::complete_irq(irq);
            }
        }

        Some(irq.as_usize())
    }

    /// Sends an inter-processor interrupt (IPI) to the specified target CPU or all CPUs.
    fn send_ipi(_irq_num: usize, target: IpiTarget) {
        match target {
            IpiTarget::Current { cpu_id } | IpiTarget::Other { cpu_id } => send_ipi(cpu_id),
            IpiTarget::AllExceptCurrent { cpu_id, cpu_num } => {
                for target_cpu in 0..cpu_num {
                    if target_cpu != cpu_id {
                        send_ipi(target_cpu);
                    }
                }
            }
        }
    }
}
