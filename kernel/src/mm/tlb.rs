#[cfg(target_arch = "loongarch64")]
mod loongarch64 {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use axhal::mem::{PhysAddr, VirtAddr};

    static ACTIVE_PAGE_TABLE_ROOTS: [AtomicUsize; axconfig::plat::CPU_NUM] =
        [const { AtomicUsize::new(0) }; axconfig::plat::CPU_NUM];

    pub(crate) fn note_page_table_switch(root_paddr: PhysAddr) {
        let cpu = axhal::percpu::this_cpu_id();
        ACTIVE_PAGE_TABLE_ROOTS[cpu].store(root_paddr.as_usize(), Ordering::Release);
    }

    pub(crate) fn flush_tlb_on_active_cpus(root_paddr: PhysAddr, vaddr: Option<VirtAddr>) {
        let root = root_paddr.as_usize();
        let flush_all_roots = root == 0;

        crate::ipi::run_on_selected_cpus(
            move |cpu| {
                flush_all_roots || ACTIVE_PAGE_TABLE_ROOTS[cpu].load(Ordering::Acquire) == root
            },
            move || axhal::asm::flush_tlb(vaddr),
        );
    }
}

#[cfg(target_arch = "loongarch64")]
pub(crate) use loongarch64::{
    flush_tlb_on_active_cpus as flush_tlb_on_all_cpus, note_page_table_switch,
};

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

    fn remote_sfence_vma(hart_mask: usize, hart_mask_base: usize, start: usize, size: usize) {
        let result = sbi_rt::remote_sfence_vma(
            HartMask::from_mask_base(hart_mask, hart_mask_base),
            start,
            size,
        );
        assert!(!result.is_err(), "SBI remote SFENCE.VMA failed: {result:?}");
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

        let cpu_num = axhal::cpu_num();
        let mask_width = usize::BITS as usize;
        let (start, size) = vaddr.map_or((0, usize::MAX), |addr| (addr.as_usize(), PAGE_SIZE_4K));

        // Follow Linux's SBI v0.2 RFENCE batching: select CPUs in the logical
        // namespace, translate each one to a firmware hart ID, then emit one
        // or more XLEN-wide HartMasks. Hart IDs need not be dense or ordered;
        // in particular, the OpenSBI boot hart occupies logical CPU slot zero.
        let mut hart_mask = 0usize;
        let mut hart_mask_base = 0usize;
        let mut highest_hart = 0usize;

        for cpu in 0..cpu_num {
            if cpu == current
                || (!flush_all_roots
                    && ACTIVE_PAGE_TABLE_ROOTS[cpu].load(Ordering::Acquire) != root)
            {
                continue;
            }

            let hart_id = axhal::cpu_hardware_id(cpu);
            if hart_mask != 0 {
                let too_far_below = hart_id < highest_hart && highest_hart - hart_id >= mask_width;
                let too_far_above =
                    hart_id >= hart_mask_base && hart_id - hart_mask_base >= mask_width;
                if too_far_below || too_far_above {
                    remote_sfence_vma(hart_mask, hart_mask_base, start, size);
                    hart_mask = 0;
                } else if hart_id < hart_mask_base {
                    hart_mask <<= hart_mask_base - hart_id;
                    hart_mask_base = hart_id;
                }
            }

            if hart_mask == 0 {
                hart_mask_base = hart_id;
                highest_hart = hart_id;
            } else {
                highest_hart = highest_hart.max(hart_id);
            }
            hart_mask |= 1usize << (hart_id - hart_mask_base);
        }

        if hart_mask != 0 {
            remote_sfence_vma(hart_mask, hart_mask_base, start, size);
        }
    }
}

#[cfg(target_arch = "riscv64")]
use riscv64::{flush_tlb_on_active_cpus, note_page_table_switch};

#[cfg(target_arch = "riscv64")]
pub(crate) fn flush_tlb_on_all_cpus(
    root_paddr: axhal::mem::PhysAddr,
    vaddr: Option<axhal::mem::VirtAddr>,
) {
    // RISC-V page faults run with S-mode interrupts disabled. SBI RFENCE is
    // synchronous in firmware and therefore does not depend on target CPUs
    // taking a supervisor IPI while the caller waits.
    flush_tlb_on_active_cpus(root_paddr, vaddr);
}

pub fn init() {
    #[cfg(target_arch = "riscv64")]
    assert!(
        axhal::cpu_num() == 1 || sbi_rt::probe_extension(sbi_rt::Fence).is_available(),
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

    #[cfg(target_arch = "loongarch64")]
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
