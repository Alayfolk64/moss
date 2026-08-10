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
        let (start, size) = vaddr.map_or((0, usize::MAX), |addr| (addr.as_usize(), PAGE_SIZE_4K));

        for cpu in 0..cpu_num {
            if cpu == current
                || (!flush_all_roots
                    && ACTIVE_PAGE_TABLE_ROOTS[cpu].load(Ordering::Acquire) != root)
            {
                continue;
            }

            // SBI masks address physical hart IDs, whereas Moss schedules with
            // dense logical CPU IDs. They differ on JH7110 because hart 0 is a
            // non-S-mode management core.
            let hart_id = axhal::cpu_hardware_id(cpu);
            let result =
                sbi_rt::remote_sfence_vma(HartMask::from_mask_base(1, hart_id), start, size);
            assert!(!result.is_err(), "SBI remote SFENCE.VMA failed: {result:?}");
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
