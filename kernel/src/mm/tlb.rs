use axhal::mem::VirtAddr;

#[cfg(target_arch = "loongarch64")]
fn flush_tlb_on_all_cpus(vaddr: Option<VirtAddr>) {
    crate::ipi::run_on_all_cpus(move || {
        axhal::asm::flush_tlb(vaddr);
    });
}

#[cfg(target_arch = "riscv64")]
mod riscv64 {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use axhal::mem::{PhysAddr, VirtAddr};
    use memory_addr::PAGE_SIZE_4K;
    use sbi_rt::HartMask;

    static ACTIVE_PAGE_TABLE_ROOTS: [AtomicUsize; axconfig::plat::CPU_NUM] =
        [const { AtomicUsize::new(0) }; axconfig::plat::CPU_NUM];

    pub(super) fn note_page_table_switch(root_paddr: PhysAddr) {
        let cpu = axhal::percpu::this_cpu_id();
        ACTIVE_PAGE_TABLE_ROOTS[cpu].store(root_paddr.as_usize(), Ordering::Release);
    }

    pub(super) fn flush_tlb_on_active_cpus(root_paddr: PhysAddr, vaddr: Option<VirtAddr>) {
        // Moss currently uses ASID 0 and performs a local full TLB flush
        // whenever a CPU changes page-table roots. Therefore a CPU can retain
        // translations for `root` only while this array says that `root` is
        // active there. A CPU racing into the address space publishes `root`
        // before changing SATP and then flushes locally; a CPU racing out can
        // be included unnecessarily, but cannot be missed while using user
        // translations from the old root.
        let root = root_paddr.as_usize();
        let flush_all_roots = root == 0;
        let current = axhal::percpu::this_cpu_id();

        if flush_all_roots || ACTIVE_PAGE_TABLE_ROOTS[current].load(Ordering::Acquire) == root {
            axhal::asm::flush_tlb(vaddr);
        }

        let cpu_num = axconfig::plat::CPU_NUM;
        let mask_width = usize::BITS as usize;
        let (start, size) = vaddr.map_or((0, usize::MAX), |addr| (addr.as_usize(), PAGE_SIZE_4K));

        for base in (0..cpu_num).step_by(mask_width) {
            let count = (cpu_num - base).min(mask_width);
            let mut mask = 0usize;
            for cpu in base..base + count {
                if cpu != current
                    && (flush_all_roots
                        || ACTIVE_PAGE_TABLE_ROOTS[cpu].load(Ordering::Acquire) == root)
                {
                    mask |= 1usize << (cpu - base);
                }
            }
            if mask == 0 {
                continue;
            }

            let result =
                sbi_rt::remote_sfence_vma(HartMask::from_mask_base(mask, base), start, size);
            assert!(!result.is_err(), "SBI remote SFENCE.VMA failed: {result:?}");
        }
    }
}

#[cfg(target_arch = "riscv64")]
use riscv64::{flush_tlb_on_active_cpus, note_page_table_switch};

#[cfg(target_arch = "riscv64")]
fn flush_tlb_on_all_cpus(root_paddr: axhal::mem::PhysAddr, vaddr: Option<VirtAddr>) {
    // RISC-V page faults run with S-mode interrupts disabled. SBI RFENCE is
    // synchronous in firmware and therefore does not depend on target CPUs
    // taking a supervisor IPI while the caller waits.
    flush_tlb_on_active_cpus(root_paddr, vaddr);
}

pub fn init() {
    #[cfg(target_arch = "riscv64")]
    assert!(
        axconfig::plat::CPU_NUM == 1 || sbi_rt::probe_extension(sbi_rt::Fence).is_available(),
        "RISC-V SMP requires the SBI RFENCE extension"
    );

    #[cfg(target_arch = "riscv64")]
    {
        note_page_table_switch(axhal::asm::read_user_page_table());
        assert!(
            axcpu::asm::register_page_table_switch_handler(note_page_table_switch),
            "page-table switch handler is already installed"
        );
    }

    assert!(
        axhal::paging::register_tlb_flush_handler(flush_tlb_on_all_cpus),
        "TLB flush handler is already installed"
    );
}
