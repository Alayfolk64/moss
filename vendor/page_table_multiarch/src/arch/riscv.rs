//! RISC-V specific page table structures.

use core::sync::atomic::{AtomicUsize, Ordering};

use memory_addr::{PhysAddr, VirtAddr};
use page_table_entry::riscv::Rv64PTE;

use crate::{PageTable64, PageTable64Mut, PagingMetaData};

/// Callback used to flush RISC-V TLBs on every active CPU.
pub type TlbFlushHandler = fn(PhysAddr, Option<VirtAddr>);

static TLB_FLUSH_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// Installs the system-wide TLB flush handler.
///
/// Returns `false` if a handler was installed already.
pub fn register_tlb_flush_handler(handler: TlbFlushHandler) -> bool {
    TLB_FLUSH_HANDLER
        .compare_exchange(0, handler as usize, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn flush_tlb_current(vaddr: Option<VirtAddr>) {
    if let Some(vaddr) = vaddr {
        riscv::asm::sfence_vma(0, vaddr.as_usize())
    } else {
        riscv::asm::sfence_vma_all();
    }
}

/// A virtual address that can be used in RISC-V Sv39 and Sv48 page tables.
pub trait SvVirtAddr: memory_addr::MemoryAddr + Send + Sync {
    /// Flush the TLB.
    fn flush_tlb(vaddr: Option<Self>);

    /// Flush the TLB only on the current CPU.
    fn flush_tlb_local(vaddr: Option<Self>) {
        Self::flush_tlb(vaddr);
    }

    /// Flush translations associated with one page-table root.
    fn flush_tlb_for(root_paddr: PhysAddr, vaddr: Option<Self>);
}

impl SvVirtAddr for VirtAddr {
    #[inline]
    fn flush_tlb(vaddr: Option<Self>) {
        let handler = TLB_FLUSH_HANDLER.load(Ordering::Acquire);
        if handler == 0 {
            flush_tlb_current(vaddr);
        } else {
            // SAFETY: only `register_tlb_flush_handler` writes this value.
            let handler = unsafe { core::mem::transmute::<usize, TlbFlushHandler>(handler) };
            // A zero root marks callers that cannot identify an address
            // space. The OS handler must conservatively flush every CPU.
            handler(0usize.into(), vaddr);
        }
    }

    #[inline]
    fn flush_tlb_local(vaddr: Option<Self>) {
        flush_tlb_current(vaddr);
    }

    #[inline]
    fn flush_tlb_for(root_paddr: PhysAddr, vaddr: Option<Self>) {
        let handler = TLB_FLUSH_HANDLER.load(Ordering::Acquire);
        if handler == 0 {
            flush_tlb_current(vaddr);
        } else {
            // SAFETY: only `register_tlb_flush_handler` writes this value.
            let handler = unsafe { core::mem::transmute::<usize, TlbFlushHandler>(handler) };
            handler(root_paddr, vaddr);
        }
    }
}

/// Metadata of RISC-V Sv39 page tables.
pub struct Sv39MetaData<VA: SvVirtAddr> {
    _virt_addr: core::marker::PhantomData<VA>,
}

/// Metadata of RISC-V Sv48 page tables.
pub struct Sv48MetaData<VA: SvVirtAddr> {
    _virt_addr: core::marker::PhantomData<VA>,
}

impl<VA: SvVirtAddr> PagingMetaData for Sv39MetaData<VA> {
    const LEVELS: usize = 3;
    const PA_MAX_BITS: usize = 56;
    const VA_MAX_BITS: usize = 39;
    type VirtAddr = VA;

    #[inline]
    fn flush_tlb(vaddr: Option<VA>) {
        <VA as SvVirtAddr>::flush_tlb(vaddr);
    }

    #[inline]
    fn flush_tlb_local(vaddr: Option<VA>) {
        <VA as SvVirtAddr>::flush_tlb_local(vaddr);
    }

    #[inline]
    fn flush_tlb_for(root_paddr: PhysAddr, vaddr: Option<VA>) {
        <VA as SvVirtAddr>::flush_tlb_for(root_paddr, vaddr);
    }
}

impl<VA: SvVirtAddr> PagingMetaData for Sv48MetaData<VA> {
    const LEVELS: usize = 4;
    const PA_MAX_BITS: usize = 56;
    const VA_MAX_BITS: usize = 48;
    type VirtAddr = VA;

    #[inline]
    fn flush_tlb(vaddr: Option<VA>) {
        <VA as SvVirtAddr>::flush_tlb(vaddr);
    }

    #[inline]
    fn flush_tlb_local(vaddr: Option<VA>) {
        <VA as SvVirtAddr>::flush_tlb_local(vaddr);
    }

    #[inline]
    fn flush_tlb_for(root_paddr: PhysAddr, vaddr: Option<VA>) {
        <VA as SvVirtAddr>::flush_tlb_for(root_paddr, vaddr);
    }
}

/// Sv39: Page-Based 39-bit (3 levels) Virtual-Memory System.
pub type Sv39PageTable<H> = PageTable64<Sv39MetaData<VirtAddr>, Rv64PTE, H>;
/// Mutable reference to a RISC-V Sv39 page table.
pub type Sv39PageTableMut<'a, H> = PageTable64Mut<'a, Sv39MetaData<VirtAddr>, Rv64PTE, H>;

/// Sv48: Page-Based 48-bit (4 levels) Virtual-Memory System.
pub type Sv48PageTable<H> = PageTable64<Sv48MetaData<VirtAddr>, Rv64PTE, H>;
/// Mutable reference to a RISC-V Sv48 page table.
pub type Sv48PageTableMut<'a, H> = PageTable64Mut<'a, Sv48MetaData<VirtAddr>, Rv64PTE, H>;
