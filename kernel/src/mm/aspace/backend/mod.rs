//! Memory mapping backends.
use alloc::{boxed::Box, sync::Arc, vec::Vec};
#[cfg(target_arch = "riscv64")]
use core::arch::asm;

use axalloc::{UsageKind, global_allocator};
use axerrno::{AxError, AxResult};
use axfs::FolioPageRef;
use axhal::{
    mem::{phys_to_virt, virt_to_phys},
    paging::{MappingFlags, PageSize, PageTable, PageTableMut},
};
use enum_dispatch::enum_dispatch;
use memory_addr::{DynPageIter, PAGE_SIZE_4K, PhysAddr, VirtAddr, VirtAddrRange};
use memory_set::MappingBackend;
#[cfg(target_arch = "riscv64")]
use spin::Once;

mod cow;
mod file;
mod linear;
mod shared;

pub use self::shared::SharedPages;
pub(crate) use self::{
    cow::{CowFaultPlan, init_frame_ref_table},
    file::FileFaultPlan,
    shared::SharedFaultPlan,
};
use super::{AddrSpace, AddrSpaceLock};

/// Result of the short, locked phase of a page fault.
pub(crate) enum FaultPreparation {
    /// The mapping already satisfies the faulting access.
    Handled,
    /// A private page can be prepared without holding the address-space lock.
    Cow(CowFaultPlan),
    /// A shared file folio can be read without holding the address-space lock.
    File(FileFaultPlan),
    /// A shared anonymous page can be prepared without holding the address-space lock.
    Shared(SharedFaultPlan),
    /// This fault still needs the regular locked backend path.
    Fallback,
}

/// Result of revalidating a page prepared outside the address-space lock.
pub(crate) enum FaultCommit {
    Handled,
    Stale,
}

fn divide_page(size: usize, page_size: PageSize) -> usize {
    assert!(page_size.is_aligned(size), "unaligned");
    size >> (page_size as usize).trailing_zeros()
}

#[cfg(target_arch = "riscv64")]
fn cboz_block_size() -> Option<usize> {
    static CBOZ_BLOCK_SIZE: Once<Option<usize>> = Once::new();

    *CBOZ_BLOCK_SIZE.call_once(|| {
        let fdt = axhal::dtb::get_fdt()?;
        let mut block_size = None;
        let mut cpu_count = 0;

        for cpu in fdt.all_nodes().filter(|node| {
            node.find_property("device_type")
                .is_some_and(|property| property.str() == "cpu")
        }) {
            let has_zicboz = cpu
                .find_property("riscv,isa-extensions")
                .is_some_and(|property| property.str_list().any(|extension| extension == "zicboz"));
            if !has_zicboz {
                return None;
            }

            let cpu_block_size = cpu.find_property("riscv,cboz-block-size")?.u32() as usize;
            if !cpu_block_size.is_power_of_two() || cpu_block_size > PAGE_SIZE_4K {
                return None;
            }
            if block_size.is_some_and(|size| size != cpu_block_size) {
                return None;
            }

            block_size = Some(cpu_block_size);
            cpu_count += 1;
        }

        (cpu_count != 0).then_some(block_size).flatten()
    })
}

/// Clears a frame before it becomes visible through a user page table.
///
/// The fixed RISC-V QEMU target advertises Zicboz and its cache-block size in
/// the device tree. Like Linux `clear_page`, use `cbo.zero` only after
/// validating that every CPU supports one common block size. Other platforms,
/// malformed device trees and unsuitable alignments retain the byte-accurate
/// fallback.
unsafe fn zero_fresh_frame(frame: PhysAddr, size: usize) {
    let start = phys_to_virt(frame).as_usize();

    #[cfg(target_arch = "riscv64")]
    if let Some(block_size) = cboz_block_size()
        && start.is_multiple_of(block_size)
        && size.is_multiple_of(block_size)
    {
        let end = start.checked_add(size).expect("frame range overflow");
        let mut addr = start;
        while addr < end {
            // CBO.ZERO is encoded as a MISC-MEM I-type instruction with
            // funct3=2 and imm=4. The generic encoding keeps the kernel target
            // at rv64gc while runtime DT validation gates its execution.
            unsafe {
                asm!(
                    ".insn i 0x0f, 2, x0, {addr}, 4",
                    addr = in(reg) addr,
                    options(nostack)
                )
            };
            addr += block_size;
        }
        return;
    }

    unsafe { core::ptr::write_bytes(start as *mut u8, 0, size) };
}

fn alloc_frame(zeroed: bool, size: PageSize) -> AxResult<PhysAddr> {
    let page_size = size as usize;
    let num_pages = page_size / PAGE_SIZE_4K;
    let frame = if size == PageSize::Size4K {
        let vaddr = VirtAddr::from(
            global_allocator()
                .alloc_page_cached(
                    axhal::percpu::this_cpu_id(),
                    axhal::cpu_num(),
                    UsageKind::VirtMem,
                )
                .map_err(|_| AxError::NoMemory)?,
        );
        virt_to_phys(vaddr)
    } else {
        let vaddr = VirtAddr::from(
            global_allocator()
                .alloc_pages(num_pages, page_size, UsageKind::VirtMem)
                .map_err(|_| AxError::NoMemory)?,
        );
        virt_to_phys(vaddr)
    };
    if zeroed {
        unsafe { zero_fresh_frame(frame, page_size) };
    }
    Ok(frame)
}

fn dealloc_frame(frame: PhysAddr, align: PageSize) {
    let vaddr = phys_to_virt(frame);
    if align == PageSize::Size4K {
        global_allocator().dealloc_page_cached(
            axhal::percpu::this_cpu_id(),
            axhal::cpu_num(),
            vaddr.as_usize(),
            UsageKind::VirtMem,
        );
    } else {
        let page_size: usize = align.into();
        let num_pages = page_size / PAGE_SIZE_4K;
        global_allocator().dealloc_pages(vaddr.as_usize(), num_pages, UsageKind::VirtMem);
    }
}

fn pages_in(range: VirtAddrRange, align: PageSize) -> AxResult<DynPageIter<VirtAddr>> {
    DynPageIter::new(range.start, range.end, align as usize).ok_or(AxError::InvalidInput)
}

/// Number of resident PTEs removed before one TLB flush and deferred release.
///
/// The gather counts actual mappings rather than virtual range size, so sparse
/// address spaces do not reserve or flush empty pages.
const UNMAP_GATHER_PAGES: usize = 8192;

enum DeferredUnmapPage {
    Anonymous {
        paddr: PhysAddr,
        page_size: PageSize,
    },
    File(FolioPageRef),
}

/// Physical-page owners waiting for the corresponding stale TLB entries to be
/// invalidated.
///
/// A normal `munmap` creates one local gather. `AddrSpace::clear` shares one
/// gather across all VMAs, avoiding a full remote TLB flush at every Cow/File
/// VMA boundary while retaining a fixed memory bound.
pub(crate) struct UnmapGather {
    pages: Vec<DeferredUnmapPage>,
}

impl UnmapGather {
    pub(crate) const fn new() -> Self {
        Self { pages: Vec::new() }
    }

    fn push_anonymous(&mut self, paddr: PhysAddr, page_size: PageSize, pt: &mut PageTableMut) {
        self.pages
            .push(DeferredUnmapPage::Anonymous { paddr, page_size });
        self.finish_if_full(pt);
    }

    fn push_file(&mut self, page: FolioPageRef, pt: &mut PageTableMut) {
        self.pages.push(DeferredUnmapPage::File(page));
        self.finish_if_full(pt);
    }

    fn finish_if_full(&mut self, pt: &mut PageTableMut) {
        if self.pages.len() >= UNMAP_GATHER_PAGES {
            self.finish(pt);
        }
    }

    /// Invalidates every queued PTE before releasing the physical-page owners.
    pub(crate) fn finish(&mut self, pt: &mut PageTableMut) {
        // Always finish the page-table cursor, including an error that cleared
        // a PTE before it could add an owner to the gather.
        pt.finish();
        for page in self.pages.drain(..) {
            match page {
                DeferredUnmapPage::Anonymous { paddr, page_size } => {
                    cow::drop_referenced_frame(paddr, page_size);
                }
                DeferredUnmapPage::File(page) => drop(page),
            }
        }
    }
}

type PopulateCallback = Box<dyn FnOnce(&mut AddrSpace)>;

#[enum_dispatch]
pub trait BackendOps {
    /// Returns the page size of the backend.
    fn page_size(&self) -> PageSize;

    /// Map a memory region.
    fn map(&self, range: VirtAddrRange, flags: MappingFlags, pt: &mut PageTableMut) -> AxResult;

    /// Unmap a memory region.
    fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult;

    /// Discard resident pages while retaining the virtual memory area.
    fn discard(&self, _range: VirtAddrRange, _pt: &mut PageTableMut) -> AxResult {
        Err(AxError::OperationNotSupported)
    }

    /// Called before a memory region is protected.
    fn on_protect(
        &self,
        _range: VirtAddrRange,
        _new_flags: MappingFlags,
        _pt: &mut PageTableMut,
    ) -> AxResult {
        Ok(())
    }

    /// Populate a memory region and return how many pages now satisfy
    /// `access_flags`.
    ///
    /// If another thread has already mapped the page with sufficient permissions,
    /// treat it as populated.
    fn populate(
        &self,
        _range: VirtAddrRange,
        _flags: MappingFlags,
        _access_flags: MappingFlags,
        _pt: &mut PageTableMut,
    ) -> AxResult<(usize, Option<PopulateCallback>)> {
        Ok((0, None))
    }

    /// Duplicates this mapping for use in a different page table.
    ///
    /// This differs from `clone`, which is designed for splitting a mapping
    /// within the same table.
    ///
    /// [`BackendOps::map`] will be latter called to the returned backend.
    fn clone_map(
        &self,
        range: VirtAddrRange,
        flags: MappingFlags,
        old_pt: &mut PageTableMut,
        new_pt: &mut PageTableMut,
        new_aspace: &Arc<AddrSpaceLock>,
    ) -> AxResult<Backend>;
}

/// A unified enum type for different memory mapping backends.
#[derive(Clone)]
#[enum_dispatch(BackendOps)]
pub enum Backend {
    Linear(linear::LinearBackend), // 线性映射，虚拟地址和物理地址有固定偏移关系
    Cow(cow::CowBackend),          // Copy-on-write，写时复制
    Shared(shared::SharedBackend), // 共享内存
    File(file::FileBackend),       // 文件映射
}

impl Backend {
    /// Unmaps one VMA as part of destroying an entire address space.
    ///
    /// Cow and file backends transfer their PTE-owned references into the
    /// address-space-wide gather. Backends without per-PTE ownership use their
    /// normal unmap implementation while sharing the same page-table cursor.
    pub(crate) fn unmap_for_clear(
        &self,
        range: VirtAddrRange,
        pt: &mut PageTableMut,
        gather: &mut UnmapGather,
    ) -> AxResult {
        match self {
            Self::Cow(backend) => backend.unmap_present_into(range, pt, gather),
            Self::File(backend) => backend.unmap_present_into(range, pt, gather),
            Self::Linear(backend) => BackendOps::unmap(backend, range, pt),
            Self::Shared(backend) => BackendOps::unmap(backend, range, pt),
        }
    }

    pub(crate) fn default_vma_kind(&self) -> (bool, bool, bool) {
        match self {
            Self::Linear(_) => (false, false, false),
            Self::Cow(backend) => {
                let file_backed = backend.file_backed();
                (false, file_backed, !file_backed)
            }
            Self::Shared(_) => (true, false, true),
            Self::File(_) => (true, true, false),
        }
    }

    pub(crate) fn merge_compatible(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Linear(left), Self::Linear(right)) => left.merge_compatible(right),
            (Self::Cow(left), Self::Cow(right)) => left.merge_compatible(right),
            (Self::Shared(left), Self::Shared(right)) => left.merge_compatible(right),
            (Self::File(left), Self::File(right)) => left.merge_compatible(right),
            _ => false,
        }
    }

    pub(crate) fn prepare_fault(
        &self,
        vaddr: VirtAddr,
        area_range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> FaultPreparation {
        match self {
            Self::Cow(backend) => backend.prepare_fault(vaddr, area_range, flags, access_flags, pt),
            Self::File(backend) => {
                backend.prepare_fault(vaddr, area_range, flags, access_flags, pt)
            }
            Self::Shared(backend) => {
                backend.prepare_fault(vaddr, area_range, flags, access_flags, pt)
            }
            _ => FaultPreparation::Fallback,
        }
    }

    pub(crate) fn commit_fault(
        &self,
        plan: &mut CowFaultPlan,
        area_range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> AxResult<FaultCommit> {
        match self {
            Self::Cow(backend) => backend.commit_fault(plan, area_range, flags, access_flags, pt),
            _ => Ok(FaultCommit::Stale),
        }
    }

    pub(crate) fn commit_file_fault(
        &self,
        plan: &mut FileFaultPlan,
        area_range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> AxResult<FaultCommit> {
        match self {
            Self::File(backend) => backend.commit_fault(plan, area_range, flags, access_flags, pt),
            _ => Ok(FaultCommit::Stale),
        }
    }

    pub(crate) fn commit_shared_fault(
        &self,
        plan: &mut SharedFaultPlan,
        area_range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> AxResult<FaultCommit> {
        match self {
            Self::Shared(backend) => {
                backend.commit_fault(plan, area_range, flags, access_flags, pt)
            }
            _ => Ok(FaultCommit::Stale),
        }
    }
}

impl MappingBackend for Backend {
    type Addr = VirtAddr;
    type Flags = MappingFlags;
    type PageTable = PageTable;

    fn map(&self, start: VirtAddr, size: usize, flags: MappingFlags, pt: &mut PageTable) -> bool {
        let range = VirtAddrRange::from_start_size(start, size);
        let mut modify = pt.modify();
        if let Err(err) = BackendOps::map(self, range, flags, &mut modify) {
            warn!("Failed to map area: {:?}", err);
            false
        } else {
            true
        }
    }

    fn unmap(&self, start: VirtAddr, size: usize, pt: &mut PageTable) -> bool {
        let range = VirtAddrRange::from_start_size(start, size);
        let mut modify = pt.modify();
        if let Err(err) = BackendOps::unmap(self, range, &mut modify) {
            warn!("Failed to unmap area: {:?}", err);
            false
        } else {
            true
        }
    }

    fn protect(
        &self,
        start: Self::Addr,
        size: usize,
        new_flags: Self::Flags,
        pt: &mut Self::PageTable,
    ) -> bool {
        let range = VirtAddrRange::from_start_size(start, size);
        let mut cursor = pt.modify();
        if let Err(err) = BackendOps::on_protect(self, range, new_flags, &mut cursor) {
            warn!("Failed to protect area: {:?}", err);
            return false;
        }
        // A private mapping may still point at a file-cache page or at an
        // anonymous frame shared after fork.  Keep those PTEs read-only even
        // when mprotect() makes the VMA writable; the first store must still
        // fault so CowBackend can copy (or exclusively upgrade) the page.
        let pte_flags = if matches!(self, Self::Cow(_)) {
            new_flags - MappingFlags::WRITE
        } else {
            new_flags
        };
        cursor.protect_region(start, size, pte_flags).is_ok()
    }
}
