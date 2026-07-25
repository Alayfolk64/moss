use axhal::mem::VirtAddr;

#[cfg(target_arch = "loongarch64")]
fn flush_tlb_on_all_cpus(vaddr: Option<VirtAddr>) {
    crate::ipi::run_on_all_cpus(move || {
        axhal::asm::flush_tlb(vaddr);
    });
}

#[cfg(target_arch = "riscv64")]
fn flush_tlb_on_all_cpus(vaddr: Option<VirtAddr>) {
    use memory_addr::PAGE_SIZE_4K;
    use sbi_rt::HartMask;

    // RISC-V page faults run with S-mode interrupts disabled. SBI RFENCE is
    // synchronous in firmware and therefore does not depend on target CPUs
    // taking a supervisor IPI while the caller waits.
    axhal::asm::flush_tlb(vaddr);

    let current = axhal::percpu::this_cpu_id();
    let cpu_num = axconfig::plat::CPU_NUM;
    let mask_width = usize::BITS as usize;
    let (start, size) = vaddr.map_or((0, usize::MAX), |addr| (addr.as_usize(), PAGE_SIZE_4K));

    for base in (0..cpu_num).step_by(mask_width) {
        let count = (cpu_num - base).min(mask_width);
        let mut mask = if count == mask_width {
            usize::MAX
        } else {
            (1usize << count) - 1
        };
        if (base..base + count).contains(&current) {
            mask &= !(1usize << (current - base));
        }
        if mask == 0 {
            continue;
        }

        let result = sbi_rt::remote_sfence_vma(HartMask::from_mask_base(mask, base), start, size);
        assert!(!result.is_err(), "SBI remote SFENCE.VMA failed: {result:?}");
    }
}

pub fn init() {
    #[cfg(target_arch = "riscv64")]
    assert!(
        axconfig::plat::CPU_NUM == 1 || sbi_rt::probe_extension(sbi_rt::Fence).is_available(),
        "RISC-V SMP requires the SBI RFENCE extension"
    );

    assert!(
        axhal::paging::register_tlb_flush_handler(flush_tlb_on_all_cpus),
        "TLB flush handler is already installed"
    );
}
