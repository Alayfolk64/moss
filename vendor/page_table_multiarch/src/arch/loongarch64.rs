//! LoongArch64 specific page table structures.

use core::{
    arch::asm,
    sync::atomic::{AtomicUsize, Ordering},
};

use memory_addr::VirtAddr;
use page_table_entry::loongarch64::LA64PTE;

use crate::{PageTable64, PageTable64Mut, PagingMetaData};

/// Metadata of LoongArch64 page tables.
#[derive(Copy, Clone, Debug)]
pub struct LA64MetaData;

/// Callback used to flush LoongArch TLBs on every active CPU.
pub type TlbFlushHandler = fn(Option<VirtAddr>);

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
    unsafe {
        if let Some(vaddr) = vaddr {
            asm!("dbar 0; invtlb 0x05, $r0, {reg}", reg = in(reg) vaddr.as_usize());
        } else {
            asm!("dbar 0; invtlb 0x00, $r0, $r0");
        }
    }
}

impl LA64MetaData {
    /// PWCL(Page Walk Controller for Lower Half Address Space) CSR flags
    ///
    /// <https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html#page-walk-controller-for-lower-half-address-space>
    ///
    /// | BitRange | Name      | Value |
    /// | ----     | ----      | ----  |
    /// | 4:0      | PTBase    |    12 |
    /// | 9:5      | PTWidth   |     9 |
    /// | 14:10    | Dir1Base  |    21 |
    /// | 19:15    | Dir1Width |     9 |
    /// | 24:20    | Dir2Base  |    30 |
    /// | 29:25    | Dir2Width |     9 |
    /// | 31:30    | PTEWidth  |     0 |
    pub const PWCL_VALUE: u32 = 12 | (9 << 5) | (21 << 10) | (9 << 15) | (30 << 20) | (9 << 25);

    /// PWCH(Page Walk Controller for Higher Half Address Space) CSR flags
    ///
    /// <https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html#page-walk-controller-for-higher-half-address-space>
    ///
    /// | BitRange | Name                            | Value |
    /// | ----     | ----                            | ----  |
    /// | 5:0      | Dir3Base                        |    39 |
    /// | 11:6     | Dir3Width                       |     9 |
    /// | 17:12    | Dir4Base                        |     0 |
    /// | 23:18    | Dir4Width                       |     0 |
    /// | 24       | 0                               |     0 |
    /// | 24       | HPTW_En(CPUCFG.2.HPTW(bit24)=1) |     0 |
    /// | 31:25    | 0                               |     0 |
    pub const PWCH_VALUE: u32 = 39 | (9 << 6);
}

impl PagingMetaData for LA64MetaData {
    const LEVELS: usize = 4;
    const PA_MAX_BITS: usize = 48;
    const VA_MAX_BITS: usize = 48;
    type VirtAddr = VirtAddr;

    #[inline]
    fn flush_tlb(vaddr: Option<VirtAddr>) {
        let handler = TLB_FLUSH_HANDLER.load(Ordering::Acquire);
        if handler == 0 {
            flush_tlb_current(vaddr);
        } else {
            // SAFETY: only `register_tlb_flush_handler` writes this value.
            let handler = unsafe { core::mem::transmute::<usize, TlbFlushHandler>(handler) };
            handler(vaddr);
        }
    }

    #[inline]
    fn flush_tlb_local(vaddr: Option<VirtAddr>) {
        flush_tlb_current(vaddr);
    }
}

/// loongarch64 page table
///
/// <https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html#section-multi-level-page-table-structure-supported-by-page-walking>
///
/// 4 levels:
///
/// using page table dir3, dir2, dir1 and pt, ignore dir4
pub type LA64PageTable<H> = PageTable64<LA64MetaData, LA64PTE, H>;
/// Mutable reference to a loongarch64 page table.
pub type LA64PageTableMut<'a, H> = PageTable64Mut<'a, LA64MetaData, LA64PTE, H>;
