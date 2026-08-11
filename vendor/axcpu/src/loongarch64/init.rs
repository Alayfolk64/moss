//! Helper functions to initialize the CPU states on systems bootstrapping.

use loongArch64::register::{stlbps, tlbidx, tlbrehi, tlbrentry};
use memory_addr::PhysAddr;
use page_table_multiarch::loongarch64::LA64MetaData;

/// Initializes TLB and MMU related registers on the current CPU.
///
/// It sets the TLB Refill exception entry (`TLBRENTY`), page table root address,
/// and finally enables the mapped address translation mode.
///
/// - TLBRENTY: <https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html#tlb-refill-exception-entry-base-address>
/// - CRMD: <https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html#current-mode-information>
pub fn init_mmu(root_paddr: PhysAddr, phys_virt_offset: usize) {
    unsafe extern "C" {
        fn handle_tlb_refill();
    }

    // Configure TLB
    const PS_4K: usize = 0x0c; // Page Size 4KB
    let tlbrentry_paddr = pa!(handle_tlb_refill as usize - phys_virt_offset);
    tlbidx::set_ps(PS_4K);
    stlbps::set_ps(PS_4K);
    tlbrehi::set_ps(PS_4K);
    tlbrentry::set_tlbrentry(tlbrentry_paddr.as_usize());

    // Configure page table walking
    unsafe {
        crate::asm::write_pwc(LA64MetaData::PWCL_VALUE, LA64MetaData::PWCH_VALUE);
        crate::asm::write_kernel_page_table(root_paddr);
        crate::asm::write_user_page_table(pa!(0));
    }
    crate::asm::flush_tlb(None);

    // Match Linux SETUP_MODES in one CSR write. Besides PG=1 this clears
    // firmware-owned IE/DA/PLV/watchpoint state and establishes the coherent
    // DATF/DATM values required by software-managed TLB refill. A single write
    // avoids an invalid transient with both DA and PG cleared or set.
    unsafe {
        core::arch::asm!(
            "ori {temp}, $r0, 0xb0",
            "csrwr {temp}, 0x0",
            temp = lateout(reg) _,
            options(nostack, preserves_flags),
        );
    }
}

/// Initializes trap handling on the current CPU.
///
/// In detail, it initializes the exception vector on LoongArch64 platforms.
pub fn init_trap() {
    #[cfg(feature = "uspace")]
    crate::uspace_common::init_exception_table();
    unsafe {
        extern "C" {
            fn exception_entry_base();
        }
        // Match Linux's early LoongArch mode setup: exceptions return to PLV0
        // with interrupts enabled unless a saved trap frame says otherwise.
        // KSAVE_KSP must be empty while executing in the kernel.
        core::arch::asm!(
            include_asm_macros!(),
            "csrwr $r0, KSAVE_KSP",
            "ori {temp}, $r0, 4",
            "csrwr {temp}, LA_CSR_PRMD",
            temp = lateout(reg) _,
        );
        crate::asm::write_exception_entry_base(exception_entry_base as usize);
    }
}
