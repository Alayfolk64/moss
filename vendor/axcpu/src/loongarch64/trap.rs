use loongArch64::register::{
    badi, badv,
    estat::{self, Exception, Trap},
};
use super::context::TrapFrame;
use crate::trap::PageFaultFlags;

const PRMD_PIE: usize = 1 << 2;

struct KernelPageFaultIrqGuard {
    enabled: bool,
}

impl KernelPageFaultIrqGuard {
    fn new(prmd: usize) -> Self {
        let enabled = prmd & PRMD_PIE != 0;
        if enabled {
            crate::asm::enable_irqs();
        }
        Self { enabled }
    }
}

impl Drop for KernelPageFaultIrqGuard {
    fn drop(&mut self) {
        if self.enabled {
            crate::asm::disable_irqs();
        }
    }
}

core::arch::global_asm!(
    include_asm_macros!(),
    include_str!("trap.S"),
    trapframe_size = const (core::mem::size_of::<TrapFrame>()),
);

fn handle_breakpoint(era: &mut usize) {
    debug!("Exception(Breakpoint) @ {era:#x} ");
    *era += 4;
}

fn handle_page_fault(tf: &mut TrapFrame, access_flags: PageFaultFlags) {
    let vaddr = va!(badv::read().vaddr());
    // Exceptions clear CRMD.IE. Restore the pre-exception interrupt state while
    // resolving a kernel page fault so the handler can block, be preempted, and
    // participate in synchronous cross-CPU TLB shootdowns. The guard leaves
    // interrupts disabled for the assembly trap-return path.
    let irq_guard = KernelPageFaultIrqGuard::new(tf.prmd);
    let handled = handle_trap!(PAGE_FAULT, vaddr, access_flags);
    drop(irq_guard);

    if handled {
        return;
    }
    #[cfg(feature = "uspace")]
    if tf.fixup_exception() {
        return;
    }
    panic!(
        "Unhandled PLV0 Page Fault @ {:#x}, fault_vaddr={:#x} ({:?}):\n{:#x?}\n{}",
        tf.era,
        vaddr,
        access_flags,
        tf,
        tf.backtrace()
    );
}

fn handle_unaligned_access(tf: &mut TrapFrame, ecode: usize, esubcode: usize) {
    let fault_vaddr = badv::read().vaddr();
    let hardware_badi = badi::read().inst();
    // LA264 leaves BADI as zero for the address-not-aligned exception.  ERA
    // still names the aligned instruction word (and was the source used by
    // the original emulator), so use it when the CSR provides no instruction.
    let instruction = if hardware_badi != 0 {
        hardware_badi
    } else {
        unsafe { core::ptr::read_volatile(tf.era as *const u32) }
    };

    if let Err(error) = unsafe { tf.emulate_unaligned_at(fault_vaddr as u64, instruction) } {
        panic!(
            "Failed to emulate PLV0 unaligned access @ {:#x}: \
             fault_vaddr={:#x}, instruction={:#010x}, ecode={:#x}, esubcode={:#x}, \
             error={}\n{:#x?}",
            tf.era, fault_vaddr, instruction, ecode, esubcode, error, tf,
        );
    }
}

#[unsafe(no_mangle)]
fn loongarch64_trap_handler(tf: &mut TrapFrame) {
    let estat = estat::read();

    match estat.cause() {
        Trap::Exception(Exception::LoadPageFault)
        | Trap::Exception(Exception::PageNonReadableFault) => {
            handle_page_fault(tf, PageFaultFlags::READ)
        }
        Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::PageModifyFault) => {
            handle_page_fault(tf, PageFaultFlags::WRITE)
        }
        Trap::Exception(Exception::FetchPageFault)
        | Trap::Exception(Exception::PageNonExecutableFault) => {
            handle_page_fault(tf, PageFaultFlags::EXECUTE);
        }
        Trap::Exception(Exception::Breakpoint) => handle_breakpoint(&mut tf.era),
        Trap::Exception(Exception::AddressNotAligned) => {
            handle_unaligned_access(tf, estat.ecode(), estat.esubcode())
        }
        Trap::Interrupt(interrupt) => {
            // `Estat::cause` selects the highest-priority pending interrupt.
            // Dispatch that same vector; using `trailing_zeros` here could
            // instead service a lower-priority timer and starve an IPI.
            handle_trap!(IRQ, interrupt as usize);
        }
        Trap::Unknown if estat.ecode() == 0 && estat.is() == 0 => {
            // A level-triggered shared interrupt can be acknowledged by the
            // other CPU (or deasserted by the device) after trap entry but
            // before ESTAT is sampled here.  With ECODE=0 and no pending IS
            // bit this is a spurious interrupt, not an unknown exception.
        }
        trap => {
            let fault_vaddr = badv::read().vaddr();
            let instruction = badi::read().inst();
            panic!(
                "Unhandled trap {:?} @ {:#x}: fault_vaddr={:#x}, \
                 instruction={:#010x}, ecode={:#x}, esubcode={:#x}\n{:#x?}\n{}",
                trap,
                tf.era,
                fault_vaddr,
                instruction,
                estat.ecode(),
                estat.esubcode(),
                tf,
                tf.backtrace()
            );
        }
    }
}
