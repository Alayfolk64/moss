//! Wrapper functions for assembly instructions.

use core::sync::atomic::{AtomicUsize, Ordering};

use memory_addr::{PhysAddr, VirtAddr};
use riscv::{
    asm,
    register::{satp, sstatus, stvec},
};

// RISC-V uses one SATP register for both kernel and user mappings. Keep the
// kernel root separately so a kernel task created while a user task is current
// does not inherit that user's address space.
static KERNEL_PAGE_TABLE_ROOT: AtomicUsize = AtomicUsize::new(0);

/// Callback invoked immediately before the current CPU installs a page-table
/// root. The operating system uses it to track which address space is active
/// on each CPU.
pub type PageTableSwitchHandler = fn(PhysAddr);

static PAGE_TABLE_SWITCH_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// Installs the page-table switch callback.
///
/// Returns `false` if a callback was installed already.
pub fn register_page_table_switch_handler(handler: PageTableSwitchHandler) -> bool {
    PAGE_TABLE_SWITCH_HANDLER
        .compare_exchange(0, handler as usize, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[inline]
fn notify_page_table_switch(root_paddr: PhysAddr) {
    let handler = PAGE_TABLE_SWITCH_HANDLER.load(Ordering::Acquire);
    if handler != 0 {
        // SAFETY: only `register_page_table_switch_handler` writes this value.
        let handler = unsafe { core::mem::transmute::<usize, PageTableSwitchHandler>(handler) };
        handler(root_paddr);
    }
}

/// Allows the current CPU to respond to interrupts.
#[inline]
pub fn enable_irqs() {
    unsafe { sstatus::set_sie() }
}

/// Makes the current CPU to ignore interrupts.
#[inline]
pub fn disable_irqs() {
    unsafe { sstatus::clear_sie() }
}

/// Returns whether the current CPU is allowed to respond to interrupts.
#[inline]
pub fn irqs_enabled() -> bool {
    sstatus::read().sie()
}

/// Relaxes the current CPU and waits for interrupts.
///
/// It must be called with interrupts enabled, otherwise it will never return.
#[inline]
pub fn wait_for_irqs() {
    riscv::asm::wfi()
}

/// Halt the current CPU.
#[inline]
pub fn halt() {
    disable_irqs();
    riscv::asm::wfi() // should never return
}

/// Reads the current page table root register for user space (`satp`).
///
/// RISC-V does not have a separate page table root register for user and
/// kernel space, so this operation is the same as [`read_kernel_page_table`].
///
/// Returns the physical address of the page table root.
#[inline]
pub fn read_user_page_table() -> PhysAddr {
    pa!(satp::read().ppn() << 12)
}

/// Reads the current page table root register for kernel space (`satp`).
///
/// RISC-V does not have a separate hardware register for the kernel page table,
/// so the root last installed by [`write_kernel_page_table`] is kept in
/// software.
///
/// Returns the physical address of the page table root.
#[inline]
pub fn read_kernel_page_table() -> PhysAddr {
    let root = KERNEL_PAGE_TABLE_ROOT.load(Ordering::Acquire);
    if root == 0 {
        read_user_page_table()
    } else {
        PhysAddr::from(root)
    }
}

/// Writes the register to update the current page table root for user space
/// (`satp`).
///
/// RISC-V does not have a separate page table root register for user
/// and kernel space, so this operation is the same as [`write_kernel_page_table`].
///
/// Note that the TLB is **NOT** flushed after this operation.
///
/// # Safety
///
/// This function is unsafe as it changes the virtual memory address space.
#[inline]
pub unsafe fn write_user_page_table(root_paddr: PhysAddr) {
    // Publish the destination first. A concurrent shootdown may include this
    // CPU before SATP changes, which is harmless: every real root change is
    // followed by a local full flush in TaskContext::switch_to or execve.
    // Publishing only after SATP changed would create a window in which the
    // new address space was active but absent from its shootdown set.
    notify_page_table_switch(root_paddr);
    unsafe { satp::set(satp::Mode::Sv39, 0, root_paddr.as_usize() >> 12) };
}

/// Records and installs the kernel page table root in `satp`.
///
/// RISC-V does not have a separate page table root register for user
/// and kernel space, so this operation is the same as [`write_user_page_table`].
///
/// Note that the TLB is **NOT** flushed after this operation.
///
/// # Safety
///
/// This function is unsafe as it changes the virtual memory address space.
#[inline]
pub unsafe fn write_kernel_page_table(root_paddr: PhysAddr) {
    KERNEL_PAGE_TABLE_ROOT.store(root_paddr.as_usize(), Ordering::Release);
    unsafe { write_user_page_table(root_paddr) };
}

/// Flushes the TLB.
///
/// If `vaddr` is [`None`], flushes the entire TLB. Otherwise, flushes the TLB
/// entry that maps the given virtual address.
#[inline]
pub fn flush_tlb(vaddr: Option<VirtAddr>) {
    if let Some(vaddr) = vaddr {
        asm::sfence_vma(0, vaddr.as_usize())
    } else {
        asm::sfence_vma_all();
    }
}

/// Writes the Supervisor Trap Vector Base Address register (`stvec`).
///
/// # Safety
///
/// This function is unsafe as it changes the exception handling behavior of the
/// current CPU.
#[inline]
pub unsafe fn write_trap_vector_base(stvec: usize) {
    let mut reg = stvec::read();
    reg.set_address(stvec);
    reg.set_trap_mode(stvec::TrapMode::Direct);
    unsafe { stvec::write(reg) }
}

/// Reads the thread pointer of the current CPU (`tp`).
///
/// It is used to implement TLS (Thread Local Storage).
#[inline]
pub fn read_thread_pointer() -> usize {
    let tp;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) tp) };
    tp
}

/// Writes the thread pointer of the current CPU (`tp`).
///
/// It is used to implement TLS (Thread Local Storage).
///
/// # Safety
///
/// This function is unsafe as it changes the CPU states.
#[inline]
pub unsafe fn write_thread_pointer(tp: usize) {
    unsafe { core::arch::asm!("mv tp, {}", in(reg) tp) }
}

#[cfg(feature = "uspace")]
core::arch::global_asm!(include_asm_macros!(), include_str!("user_copy.S"));

#[cfg(feature = "uspace")]
unsafe extern "C" {
    /// Copies data from source to destination, where addresses may be in user
    /// space. Equivalent to memcpy.
    ///
    /// # Safety
    /// This function is unsafe because it performs raw memory operations.
    ///
    /// # Returns
    /// Returns the number of bytes not copied. This means 0 indicates success,
    /// while a value > 0 indicates failure.
    pub fn user_copy(dst: *mut u8, src: *const u8, size: usize) -> usize;
}
