use axhal::mem::VirtAddr;

fn flush_tlb_on_all_cpus(vaddr: Option<VirtAddr>) {
    crate::ipi::run_on_all_cpus(move || {
        axhal::asm::flush_tlb(vaddr);
    });
}

pub fn init() {
    assert!(
        axhal::paging::register_tlb_flush_handler(flush_tlb_on_all_cpus),
        "TLB flush handler is already installed"
    );
}
