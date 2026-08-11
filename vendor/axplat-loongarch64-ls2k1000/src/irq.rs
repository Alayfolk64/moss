use axplat::irq::{HandlerTable, IpiTarget, IrqHandler, IrqIf};
use loongArch64::{
    iocsr::{iocsr_read_w, iocsr_write_w},
    ipi::send_ipi_single,
    register::{
        ecfg::{self, LineBasedInterrupt},
        ticlr,
    },
};

use crate::{
    config::devices::{IPI_IRQ, TIMER_IRQ},
    fdt::{
        LIOINTC_IRQ_BASE, LIOINTC_IRQS_PER_CONTROLLER, LioIntcResource, MAX_LIOINTC, resources,
        uncached_addr,
    },
    ipi_state::{ENABLED_ACTIONS, RUNTIME_ACTION, decode_pending},
    irq_state::first_pending_source,
};

const MAX_IRQ_COUNT: usize = LIOINTC_IRQ_BASE + MAX_LIOINTC * LIOINTC_IRQS_PER_CONTROLLER;
const IOCSR_IPI_STATUS: usize = 0x1000;
const IOCSR_IPI_ENABLE: usize = 0x1004;
const IOCSR_IPI_CLEAR: usize = 0x100c;

static IRQ_HANDLERS: HandlerTable<MAX_IRQ_COUNT> = HandlerTable::new();

#[derive(Clone, Copy)]
struct LioIntc {
    resource: LioIntcResource,
    main: *mut u8,
    cpu0_isr: *const u32,
}

unsafe impl Send for LioIntc {}
unsafe impl Sync for LioIntc {}

impl LioIntc {
    const ENABLE_STATUS: usize = 0x24;
    const ENABLE: usize = 0x28;
    const DISABLE: usize = 0x2c;
    const POLARITY: usize = 0x30;
    const EDGE: usize = 0x34;

    fn new(resource: LioIntcResource) -> Self {
        assert!(
            resource.regs_size >= 0x38,
            "LIOINTC register window is too small"
        );
        assert!(resource.isr_size >= 4, "LIOINTC ISR window is too small");
        Self {
            main: uncached_addr(resource.regs_paddr) as *mut u8,
            cpu0_isr: uncached_addr(resource.isr_paddr) as *const u32,
            resource,
        }
    }

    fn init(self) {
        self.write32(Self::DISABLE, u32::MAX);
        self.write32(Self::EDGE, 0);
        self.write32(Self::POLARITY, 0);
        let route = (1 << (self.resource.parent_index + 4)) | 1;
        for source in 0..LIOINTC_IRQS_PER_CONTROLLER {
            unsafe { self.main.add(source).write_volatile(route as u8) };
        }
        unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };
    }

    fn source_for_irq(self, irq: usize) -> Option<usize> {
        irq.checked_sub(self.resource.irq_base)
            .filter(|source| *source < LIOINTC_IRQS_PER_CONTROLLER)
    }

    fn set_enable(self, source: usize, enabled: bool) {
        let mask = 1_u32 << source;
        self.write32(if enabled { Self::ENABLE } else { Self::DISABLE }, mask);
        unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };
    }

    fn pending(self) -> u32 {
        unsafe { self.cpu0_isr.read_volatile() & self.read32(Self::ENABLE_STATUS) }
    }

    fn read32(self, offset: usize) -> u32 {
        unsafe { self.main.add(offset).cast::<u32>().read_volatile() }
    }

    fn write32(self, offset: usize, value: u32) {
        unsafe { self.main.add(offset).cast::<u32>().write_volatile(value) }
    }
}

fn controllers() -> impl Iterator<Item = LioIntc> {
    resources()
        .liointc
        .iter()
        .flatten()
        .copied()
        .map(LioIntc::new)
}

fn set_arch_line(irq: usize, enabled: bool) {
    let line = LineBasedInterrupt::from_bits_truncate(1usize << irq);
    let current = ecfg::read().lie();
    ecfg::set_lie(if enabled {
        current | line
    } else {
        current & !line
    });
}

pub(crate) fn init() {
    let mut count = 0;
    for controller in controllers() {
        controller.init();
        set_arch_line(controller.resource.parent_irq, true);
        count += 1;
    }
    assert!(count != 0, "no LIOINTC initialized");
    info!("LS2K1000 LIOINTC initialized from DTB: controllers={count}");
}

fn handle_liointc(controller: LioIntc) -> Option<usize> {
    let pending = controller.pending();
    let Some(source) = first_pending_source(pending) else {
        debug!("Spurious LS2K1000 LIOINTC interrupt");
        return None;
    };
    let irq = controller.resource.irq_base + source;

    // LIOINTC device inputs are level-triggered. Match Linux's
    // handle_level_irq flow: mask the source while the device handler clears
    // its status, then unmask it only after the MMIO acknowledgement has
    // completed. Without this sequence LS2K1000 can immediately re-enter the
    // parent interrupt with a stale CPU ISR bit even though the AHCI PxIS and
    // HBA IS registers already read back as zero.
    controller.set_enable(source, false);
    let handled = IRQ_HANDLERS.handle(irq);
    if handled {
        controller.set_enable(source, true);
    } else {
        warn!("Unhandled LS2K1000 device IRQ {irq}; masked source {source}");
    }
    // Return the exact IRQ whose handler ran. Other pending level sources
    // remain asserted and retrigger, allowing the generic post-IRQ hook to
    // wake the correct per-device PollSet for every source.
    Some(irq)
}

fn send_runtime_ipi(cpu_id: usize) {
    let hardware_id = crate::fdt::logical_cpu_hardware_id(cpu_id);
    send_ipi_single(hardware_id, RUNTIME_ACTION);
}

fn handle_ipi() {
    let status = iocsr_read_w(IOCSR_IPI_STATUS);
    if status == 0 {
        return;
    }

    // Linux acknowledges every latched action before dispatch. In particular,
    // the boot action has no runtime callback behind it and must be ignored.
    iocsr_write_w(IOCSR_IPI_CLEAR, status);
    unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };

    let pending = decode_pending(status);
    if pending.runtime {
        if !IRQ_HANDLERS.handle(IPI_IRQ) {
            debug!("Unhandled IPI");
        }
    }
    if pending.unknown != 0 {
        warn!("Unsupported LS2K1000 IPI actions: {:#x}", pending.unknown);
    }
}

struct IrqIfImpl;

#[impl_plat_interface]
impl IrqIf for IrqIfImpl {
    fn set_enable(irq: usize, enabled: bool) {
        if irq == TIMER_IRQ {
            set_arch_line(TIMER_IRQ, enabled);
            return;
        }
        if irq == IPI_IRQ {
            iocsr_write_w(IOCSR_IPI_ENABLE, if enabled { ENABLED_ACTIONS } else { 0 });
            set_arch_line(IPI_IRQ, enabled);
            return;
        }
        for controller in controllers() {
            if let Some(source) = controller.source_for_irq(irq) {
                controller.set_enable(source, enabled);
                return;
            }
        }
        warn!("request for unknown LS2K1000 IRQ {irq}");
    }

    fn register(irq: usize, handler: IrqHandler) -> bool {
        if irq >= MAX_IRQ_COUNT || !IRQ_HANDLERS.register_handler(irq, handler) {
            return false;
        }
        Self::set_enable(irq, true);
        true
    }

    fn unregister(irq: usize) -> Option<IrqHandler> {
        IRQ_HANDLERS
            .unregister_handler(irq)
            .inspect(|_| Self::set_enable(irq, false))
    }

    fn handle(irq: usize) -> Option<usize> {
        if irq == TIMER_IRQ {
            ticlr::clear_timer_interrupt();
            IRQ_HANDLERS.handle(TIMER_IRQ);
            return Some(TIMER_IRQ);
        }
        if irq == IPI_IRQ {
            handle_ipi();
            return Some(IPI_IRQ);
        }
        for controller in controllers() {
            if controller.resource.parent_irq == irq {
                return handle_liointc(controller);
            }
        }
        debug!("Unhandled LS2K1000 architectural IRQ {irq}");
        None
    }

    fn send_ipi(_irq_num: usize, target: IpiTarget) {
        match target {
            IpiTarget::Current { cpu_id } | IpiTarget::Other { cpu_id } => send_runtime_ipi(cpu_id),
            IpiTarget::AllExceptCurrent { cpu_id, cpu_num } => {
                for target in 0..cpu_num {
                    if target != cpu_id {
                        send_runtime_ipi(target);
                    }
                }
            }
        }
    }
}
