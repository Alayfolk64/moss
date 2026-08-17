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

/// Synchronizes subsequent instruction fetches with earlier stores performed
/// by this hart. Debuggers use this after replacing user text with a software
/// breakpoint and immediately before the tracee returns to user space.
#[inline]
pub fn flush_icache_all() {
    unsafe { core::arch::asm!("fence.i", options(nostack, preserves_flags)) }
}

/// Halt the current CPU.
#[inline]
pub fn halt() {
    disable_irqs();
    riscv::asm::wfi() // should never return
}

/// Reads the current page table root register for user space (`satp`).
///
/// RISC-V uses the same hardware register for temporary boot, user, and kernel
/// page tables. This operation reports the root active on the current CPU;
/// [`read_kernel_page_table`] instead reports the globally published final
/// kernel root.
///
/// Returns the physical address of the page table root.
#[inline]
pub fn read_user_page_table() -> PhysAddr {
    pa!(satp::read().ppn() << 12)
}

/// Reads the globally published kernel page table root.
///
/// RISC-V does not have a separate hardware register for the kernel page table,
/// so the final kernel root is published once in software before kernel tasks
/// are created. The temporary boot page table is deliberately not visible
/// through this interface.
///
/// Returns the physical address of the page table root.
#[inline]
pub fn read_kernel_page_table() -> PhysAddr {
    let root = KERNEL_PAGE_TABLE_ROOT.load(Ordering::Acquire);
    assert_ne!(root, 0, "kernel page table root has not been published");
    PhysAddr::from(root)
}

/// Installs a temporary boot page table in the current CPU's `satp` register.
///
/// This operation neither publishes a kernel page table root nor invokes the
/// page-table switch callback. It is intended for the early boot path before
/// per-CPU state and the final kernel address space are available.
///
/// Note that the TLB is **NOT** flushed after this operation.
///
/// # Safety
///
/// The given root must map the code and data needed to continue early boot on
/// the current CPU.
#[inline]
pub unsafe fn write_boot_page_table(root_paddr: PhysAddr) {
    let root = root_paddr.as_usize();
    assert_ne!(root, 0, "boot page table root must not be zero");
    unsafe { satp::set(satp::Mode::Sv39, 0, root >> 12) };
}

/// Publishes the final kernel page table root exactly once.
///
/// This operation records the root in software but does not modify any CPU's
/// `satp` register. Only the primary CPU's memory-management initialization may
/// call it.
///
/// # Safety
///
/// The given root must remain a valid kernel page table for the rest of the
/// current boot lifecycle.
#[inline]
pub unsafe fn publish_kernel_page_table(root_paddr: PhysAddr) {
    let root = root_paddr.as_usize();
    assert_ne!(root, 0, "kernel page table root must not be zero");
    if let Err(published) =
        KERNEL_PAGE_TABLE_ROOT.compare_exchange(0, root, Ordering::Release, Ordering::Acquire)
    {
        panic!(
            "kernel page table root was already published: published={published:#x}, \
             attempted={root:#x}"
        );
    }
}

/// Writes the register to update the current page table root for user space
/// (`satp`).
///
/// Unlike [`write_kernel_page_table`], this operation accepts roots belonging
/// to user address spaces and does not require the final kernel root.
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

/// Installs the globally published kernel page table root in the current CPU's
/// `satp` register.
///
/// The root must first be published by [`publish_kernel_page_table`]. This
/// operation is local to the current CPU and does not change the published
/// root.
///
/// Note that the TLB is **NOT** flushed after this operation.
///
/// # Safety
///
/// This function is unsafe as it changes the virtual memory address space.
#[inline]
pub unsafe fn write_kernel_page_table(root_paddr: PhysAddr) {
    let published = read_kernel_page_table();
    assert_eq!(
        root_paddr, published,
        "attempted to install a page table other than the published kernel root"
    );
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
