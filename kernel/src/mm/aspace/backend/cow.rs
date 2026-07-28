use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    slice,
    sync::atomic::{AtomicBool, Ordering},
};

use axerrno::{AxError, AxResult};
use axfs::{FileBackend, FolioPageRef};
use axhal::{
    mem::{phys_ram_ranges, phys_to_virt},
    paging::{MappingFlags, PageSize, PageTable, PageTableMut, PagingError},
};
use axsync::Mutex;
use kspin::SpinNoPreempt;
use memory_addr::{MemoryAddr, PhysAddr, VirtAddr, VirtAddrRange};
use spin::Once;

use super::{
    AddrSpaceLock, Backend, BackendOps, FaultCommit, FaultPreparation, PopulateCallback,
    UnmapGather, alloc_frame, dealloc_frame, pages_in,
};

const FRAME_TABLE_SHARDS: usize = 64;
const FILE_PAGE_SHARDS: usize = 64;

#[derive(Clone, Copy)]
struct FrameRefRange {
    start_pfn: usize,
    end_pfn: usize,
    index_base: usize,
}

#[derive(Clone, Copy)]
struct FrameRefLocation {
    shard: usize,
    slot: usize,
}

/// Anonymous-frame reference counts indexed directly by physical page.
///
/// The compact logical index excludes holes between physical RAM ranges.
/// Consecutive logical pages are striped across 64 independently locked
/// vectors, preserving the old parallelism without a tree lookup per access.
struct FrameRefTable {
    ranges: Vec<FrameRefRange>,
    shards: [SpinNoPreempt<Vec<usize>>; FRAME_TABLE_SHARDS],
}

impl FrameRefTable {
    const INITIAL_COUNT: usize = 1;

    fn new() -> Self {
        let page_size = memory_addr::PAGE_SIZE_4K;
        let mut physical_ranges = phys_ram_ranges().to_vec();
        physical_ranges.sort_unstable_by_key(|&(start, _)| start);

        let mut ranges = Vec::with_capacity(physical_ranges.len());
        let mut total_pages = 0usize;
        let mut previous_end = 0usize;
        for (range_index, (start, size)) in physical_ranges.into_iter().enumerate() {
            let end = start
                .checked_add(size)
                .expect("physical RAM range overflow");
            assert_eq!(start % page_size, 0, "unaligned physical RAM start");
            assert_eq!(end % page_size, 0, "unaligned physical RAM end");
            assert!(
                range_index == 0 || start >= previous_end,
                "overlapping physical RAM ranges"
            );

            let start_pfn = start / page_size;
            let end_pfn = end / page_size;
            let page_count = end_pfn - start_pfn;
            ranges.push(FrameRefRange {
                start_pfn,
                end_pfn,
                index_base: total_pages,
            });
            total_pages = total_pages
                .checked_add(page_count)
                .expect("physical page count overflow");
            previous_end = end;
        }

        assert!(total_pages > 0, "no physical RAM for frame reference table");
        let shards = core::array::from_fn(|shard| {
            let len = total_pages
                .saturating_sub(shard)
                .div_ceil(FRAME_TABLE_SHARDS);
            SpinNoPreempt::new(alloc::vec![0; len])
        });

        Self { ranges, shards }
    }

    fn count(&self, paddr: PhysAddr) -> Option<usize> {
        self.with_count(paddr, core::convert::identity)
    }

    fn location(&self, paddr: PhysAddr) -> Option<FrameRefLocation> {
        if !paddr.is_aligned(memory_addr::PAGE_SIZE_4K) {
            return None;
        }
        let pfn = paddr.as_usize() / memory_addr::PAGE_SIZE_4K;
        let range = self
            .ranges
            .iter()
            .find(|range| pfn >= range.start_pfn && pfn < range.end_pfn)?;
        let index = range.index_base + pfn - range.start_pfn;
        Some(FrameRefLocation {
            shard: index % FRAME_TABLE_SHARDS,
            slot: index / FRAME_TABLE_SHARDS,
        })
    }

    /// Calls `f` while the frame's shard remains locked.
    ///
    /// A stable count is required by the exclusive COW permission upgrade:
    /// fork must not add a reference between validating exclusivity and
    /// changing the PTE.
    fn with_count<R>(&self, paddr: PhysAddr, f: impl FnOnce(usize) -> R) -> Option<R> {
        let location = self.location(paddr)?;
        let counts = self.shards[location.shard].lock();
        let count = counts[location.slot];
        if count == 0 { None } else { Some(f(count)) }
    }

    /// Reserves one mapping or temporary COW pin and returns the new count.
    fn reference_frame(&self, paddr: PhysAddr) -> Option<usize> {
        let location = self.location(paddr)?;
        let mut counts = self.shards[location.shard].lock();
        let count = &mut counts[location.slot];
        if *count == 0 {
            return None;
        }
        assert!(*count > 0, "referencing unreferenced frame");
        let next = count.checked_add(1)?;
        *count = next;
        Some(next)
    }

    fn init_frame(&self, paddr: PhysAddr) {
        let location = self
            .location(paddr)
            .expect("allocated frame outside physical RAM");
        let mut counts = self.shards[location.shard].lock();
        let count = &mut counts[location.slot];
        assert_eq!(*count, 0, "initializing already referenced frame");
        *count = Self::INITIAL_COUNT;
    }

    /// Releases one reference and reports whether the count reached zero.
    ///
    /// The caller deallocates the physical page only after dropping the shard
    /// lock, so the allocator can never recursively enter this frame table.
    fn release_frame(&self, paddr: PhysAddr) -> bool {
        let location = self
            .location(paddr)
            .expect("released frame outside physical RAM");
        let mut counts = self.shards[location.shard].lock();
        let count = &mut counts[location.slot];
        assert!(*count > 0, "dropping unreferenced frame");
        *count -= 1;
        *count == 0
    }
}

static FRAME_REFS: Once<FrameRefTable> = Once::new();

pub(crate) fn init_frame_ref_table() {
    FRAME_REFS.call_once(FrameRefTable::new);
}

fn frame_refs() -> &'static FrameRefTable {
    FRAME_REFS
        .get()
        .expect("frame reference table is not initialized")
}

struct FilePageShard {
    inline: Option<(VirtAddr, FolioPageRef)>,
    overflow: BTreeMap<VirtAddr, FolioPageRef>,
}

impl FilePageShard {
    fn new() -> Self {
        Self {
            inline: None,
            overflow: BTreeMap::new(),
        }
    }

    fn get(&self, vaddr: &VirtAddr) -> Option<&FolioPageRef> {
        if let Some((inline_addr, page)) = self.inline.as_ref()
            && inline_addr == vaddr
        {
            return Some(page);
        }
        self.overflow.get(vaddr)
    }

    fn insert(&mut self, vaddr: VirtAddr, page: FolioPageRef) {
        match self.inline.as_mut() {
            None => {
                debug_assert!(self.overflow.is_empty());
                self.inline = Some((vaddr, page));
            }
            Some((inline_addr, inline_page)) if *inline_addr == vaddr => {
                *inline_page = page;
            }
            Some(_) => {
                self.overflow.insert(vaddr, page);
            }
        }
    }

    fn remove(&mut self, vaddr: &VirtAddr) -> Option<FolioPageRef> {
        if self
            .inline
            .as_ref()
            .is_some_and(|(inline_addr, _)| inline_addr == vaddr)
        {
            let (_, page) = self.inline.take()?;
            self.inline = self.overflow.pop_first();
            return Some(page);
        }
        self.overflow.remove(vaddr)
    }
}

struct FilePageRefs {
    shards: [Mutex<FilePageShard>; FILE_PAGE_SHARDS],
}

impl FilePageRefs {
    fn new() -> Self {
        Self {
            shards: core::array::from_fn(|_| Mutex::new(FilePageShard::new())),
        }
    }

    fn shard(&self, vaddr: VirtAddr) -> &Mutex<FilePageShard> {
        let page = vaddr.as_usize() / memory_addr::PAGE_SIZE_4K;
        &self.shards[page % FILE_PAGE_SHARDS]
    }

    fn get(&self, vaddr: VirtAddr) -> Option<FolioPageRef> {
        self.shard(vaddr).lock().get(&vaddr).cloned()
    }

    fn insert(&self, vaddr: VirtAddr, page: FolioPageRef) {
        self.shard(vaddr).lock().insert(vaddr, page);
    }

    fn remove(&self, vaddr: VirtAddr) -> Option<FolioPageRef> {
        self.shard(vaddr).lock().remove(&vaddr)
    }
}

/// Copy-on-write mapping backend.
///
/// This corresponds to the `MAP_PRIVATE` flag.
pub struct CowBackend {
    start: VirtAddr,
    size: PageSize,
    file: Option<(FileBackend, u64, Option<u64>)>,
    /// Set after the first resident PTE is installed.
    ///
    /// This deliberately never returns to `false`: fork only needs a safe
    /// "definitely empty" fast path. An exact shared counter would make
    /// unrelated faults on different CPUs contend on one cache line.
    has_mappings: Arc<AtomicBool>,
    identity: Arc<()>,
    /// Cached file pages mapped directly into this address space. Keeping the
    /// reference here pins the frame even after it leaves the file-cache LRU.
    ///
    /// Anonymous mappings leave this as `None` and avoid allocating an empty
    /// 64-shard ownership table.
    file_pages: Option<Arc<FilePageRefs>>,
}

impl Clone for CowBackend {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            has_mappings: self.has_mappings.clone(),
            identity: self.identity.clone(),
            file_pages: self.file_pages.clone(),
        }
    }
}

impl CowBackend {
    pub(super) fn file_backed(&self) -> bool {
        self.file.is_some()
    }

    pub(super) fn merge_compatible(&self, other: &Self) -> bool {
        self.start == other.start
            && self.size == other.size
            && Arc::ptr_eq(&self.identity, &other.identity)
    }
}

/// A not-yet-mapped private page. Allocation and file I/O are deliberately
/// performed while no address-space lock is held. `Drop` releases a page that
/// loses the revalidation race.
pub(crate) struct CowFaultPlan {
    identity: Arc<()>,
    vaddr: VirtAddr,
    mapping_start: VirtAddr,
    area_range: VirtAddrRange,
    size: PageSize,
    file: Option<(FileBackend, u64, Option<u64>)>,
    access_flags: MappingFlags,
    frame: Option<PhysAddr>,
    file_pages: Vec<(VirtAddr, FolioPageRef)>,
    source: Option<CowFaultSource>,
}

enum CowFaultSource {
    /// The PTE owns the only ordinary reference. The extra plan reference
    /// keeps the frame alive while the address-space lock is released.
    Exclusive(PhysAddr),
    /// One extra COW reference pins an anonymous frame while the address-space
    /// lock is released.
    Anonymous(PhysAddr),
    /// A cloned cache reference pins a directly mapped file page.
    File(FolioPageRef),
}

impl CowFaultSource {
    fn paddr(&self) -> PhysAddr {
        match self {
            Self::Exclusive(paddr) | Self::Anonymous(paddr) => *paddr,
            Self::File(page) => page.paddr(),
        }
    }
}

impl CowFaultPlan {
    pub(crate) fn execute(&mut self) -> AxResult {
        if let Some(source) = &self.source {
            if matches!(source, CowFaultSource::Exclusive(_)) {
                return Ok(());
            }
            let frame = alloc_referenced_frame(false, self.size)?;
            self.frame = Some(frame);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(source.paddr()).as_ptr(),
                    phys_to_virt(frame).as_mut_ptr(),
                    self.size as _,
                );
            }
            return Ok(());
        }

        if let Some((file, mapping_file_start, file_end)) = &self.file {
            let start = self
                .mapping_start
                .as_usize()
                .saturating_sub(self.vaddr.as_usize());
            assert!(start < self.size as usize);

            let fault_file_start = *mapping_file_start
                + self
                    .vaddr
                    .as_usize()
                    .saturating_sub(self.mapping_start.as_usize()) as u64;
            let max_read = file_end
                .map_or(u64::MAX, |end| end.saturating_sub(fault_file_start))
                .min((self.size as usize).saturating_sub(start) as u64)
                as usize;

            // Complete aligned cached pages can be installed read-only and
            // copied only when actually written. Like Linux fault-around, the
            // faulting page demand-loads a naturally aligned folio. Install
            // ready siblings from that same folio without repeating its cache
            // lookup for every 4-KiB page.
            if self.size == PageSize::Size4K
                && !self.access_flags.contains(MappingFlags::WRITE)
                && start == 0
                && max_read == self.size as usize
            {
                let page_size = self.size as usize;
                if let Some(fault_page) = file.page_ref_at(fault_file_start)? {
                    let folio_page_offset = fault_page.folio_page_offset();
                    let window_start = VirtAddr::from(
                        self.vaddr
                            .as_usize()
                            .saturating_sub(folio_page_offset * page_size),
                    );
                    let window_end = window_start + fault_page.folio_page_count() * page_size;
                    let first = window_start
                        .max(self.area_range.start.align_up(page_size))
                        .max(self.mapping_start.align_up(page_size));
                    let last = window_end.min(self.area_range.end.align_down(page_size));

                    // Put the demanded page first so commit resolves the
                    // original fault before spending work on optional PTEs.
                    self.file_pages.push((self.vaddr, fault_page.clone()));
                    let mut addr = first;
                    while addr < last {
                        if addr != self.vaddr {
                            let offset = *mapping_file_start
                                + addr
                                    .as_usize()
                                    .saturating_sub(self.mapping_start.as_usize())
                                    as u64;
                            let complete = file_end
                                .is_none_or(|end| offset.saturating_add(page_size as u64) <= end);
                            let page_in_folio = (addr - window_start) / page_size;
                            if complete && let Some(page) = fault_page.folio_page_ref(page_in_folio)
                            {
                                self.file_pages.push((addr, page));
                            }
                        }
                        addr += page_size;
                    }
                    return Ok(());
                }
            }

            let frame = alloc_referenced_frame(true, self.size)?;
            self.frame = Some(frame);
            let buf = unsafe {
                slice::from_raw_parts_mut(phys_to_virt(frame).as_mut_ptr(), self.size as _)
            };
            file.read_at(&mut &mut buf[start..start + max_read], fault_file_start)?;
        } else {
            #[cfg(feature = "fault-repro-trace")]
            if crate::mm::is_page_fault_repro_address(self.vaddr) {
                warn!(
                    "[PFTRACE 07] execute-plan source=none backing=anonymous \
                     action=allocate-and-zero size={:#x}",
                    self.size as usize,
                );
            }
            let frame = alloc_referenced_frame(true, self.size)?;
            #[cfg(feature = "fault-repro-trace")]
            if crate::mm::is_page_fault_repro_address(self.vaddr) {
                warn!(
                    "[PFTRACE 08] execute-plan-complete zeroed_frame={frame:#x} (still not \
                     visible in page table)",
                );
            }
            self.frame = Some(frame);
        }
        Ok(())
    }
}

impl Drop for CowFaultPlan {
    fn drop(&mut self) {
        if let Some(frame) = self.frame.take() {
            drop_referenced_frame(frame, self.size);
        }
        if let Some(source) = self.source.take() {
            match source {
                CowFaultSource::Exclusive(frame) | CowFaultSource::Anonymous(frame) => {
                    drop_referenced_frame(frame, self.size);
                }
                CowFaultSource::File(_) => {}
            }
        }
    }
}

fn alloc_referenced_frame(zeroed: bool, size: PageSize) -> AxResult<PhysAddr> {
    let frame = alloc_frame(zeroed, size)?;
    frame_refs().init_frame(frame);
    Ok(frame)
}

pub(super) fn drop_referenced_frame(frame: PhysAddr, size: PageSize) {
    let deallocate = frame_refs().release_frame(frame);
    if deallocate {
        dealloc_frame(frame, size);
    }
}

impl CowBackend {
    fn commit_missing_fault(
        &self,
        plan: &mut CowFaultPlan,
        area_range: VirtAddrRange,
        flags: MappingFlags,
        pt: &PageTable,
    ) -> AxResult<FaultCommit> {
        if !plan.file_pages.is_empty() {
            let Some((file, ..)) = plan.file.as_ref() else {
                return Ok(FaultCommit::Stale);
            };
            let Some(file_page_refs) = self.file_pages.as_ref() else {
                return Ok(FaultCommit::Stale);
            };
            let pages = core::mem::take(&mut plan.file_pages);
            let validation_page = pages
                .iter()
                .find(|(addr, _)| *addr == plan.vaddr)
                .map(|(_, page)| page.clone())
                .ok_or(AxError::BadAddress)?;
            let mut installed = 0;
            let mut fault_installed = false;
            let mut fault_error = None;
            let validated = file.with_valid_folio_ref(&validation_page, || {
                for (addr, page) in pages {
                    if !area_range.contains(addr) {
                        continue;
                    }
                    let mapping_page = page.clone();
                    // The leaf CAS is both the commit-time absence check and
                    // the installation. A separate query would repeat the
                    // complete page-table walk on every successful fault.
                    let mut file_pages = file_page_refs.shard(addr).lock();
                    match pt.map_4k_concurrent(addr, page.paddr(), flags - MappingFlags::WRITE) {
                        Ok(()) => {
                            file_pages.insert(addr, mapping_page);
                            installed += 1;
                            fault_installed |= addr == plan.vaddr;
                        }
                        Err(PagingError::AlreadyMapped) => {}
                        Err(err) if addr == plan.vaddr => {
                            fault_error = Some(err);
                            break;
                        }
                        Err(_) => {}
                    }
                }
            });
            if validated.is_none() {
                return Ok(FaultCommit::Stale);
            }
            if installed != 0 {
                self.mark_mapped();
            }
            if let Some(err) = fault_error {
                return Err(err.into());
            }
            return Ok(if fault_installed {
                FaultCommit::Handled
            } else {
                FaultCommit::Stale
            });
        }

        let frame = plan.frame.ok_or(AxError::BadAddress)?;
        match pt.map_4k_concurrent(plan.vaddr, frame, flags) {
            Ok(()) => {
                #[cfg(feature = "fault-repro-trace")]
                if crate::mm::is_page_fault_repro_address(plan.vaddr) {
                    warn!(
                        "[PFTRACE 09] commit-pte va={:#x} pa={frame:#x} flags={flags:?} \
                         result=installed",
                        plan.vaddr,
                    );
                }
                plan.frame = None;
            }
            Err(PagingError::AlreadyMapped) => return Ok(FaultCommit::Stale),
            Err(err) => return Err(err.into()),
        }
        self.mark_mapped();
        Ok(FaultCommit::Handled)
    }

    fn clone_for_fork(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            has_mappings: Arc::new(AtomicBool::new(false)),
            identity: Arc::new(()),
            file_pages: self
                .file_pages
                .as_ref()
                .map(|_| Arc::new(FilePageRefs::new())),
        }
    }

    #[inline]
    fn mark_mapped(&self) {
        if !self.has_mappings.load(Ordering::Relaxed) {
            self.has_mappings.store(true, Ordering::Relaxed);
        }
    }

    fn alloc_new_frame(&self, zeroed: bool) -> AxResult<PhysAddr> {
        alloc_referenced_frame(zeroed, self.size)
    }

    fn drop_new_frame(&self, frame: PhysAddr) {
        drop_referenced_frame(frame, self.size);
    }

    fn unmap_present(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        // Most VMAs are sparse or small. Grow only when a resident PTE is
        // found instead of allocating the full gather array for every VMA.
        let mut gather = UnmapGather::new();
        let result = self.unmap_present_into(range, pt, &mut gather);
        // This must also run on errors: dropping a deferred frame before the
        // queued TLB invalidation completes would permit stale translations to
        // access a recycled physical page.
        gather.finish(pt);
        result
    }

    pub(super) fn unmap_present_into(
        &self,
        range: VirtAddrRange,
        pt: &mut PageTableMut,
        gather: &mut UnmapGather,
    ) -> AxResult {
        let mut cursor = range.start;
        let file_page_refs = self.file_pages.as_deref();

        while let Some((vaddr, paddr, _flags, page_size)) =
            pt.next_present_mapping(cursor, range.end)
        {
            if vaddr < range.start || vaddr >= range.end || page_size != self.size {
                return Err(AxError::BadAddress);
            }
            cursor = vaddr + page_size as usize;

            // Resolve the ownership record before clearing the PTE. On an
            // error the mapping remains intact and no frame reference is lost.
            let file_page = file_page_refs.and_then(|pages| pages.remove(vaddr));
            if let Some(page) = file_page.as_ref() {
                if page.paddr() != paddr {
                    file_page_refs
                        .expect("file page reference table disappeared")
                        .insert(vaddr, page.clone());
                    return Err(AxError::BadAddress);
                }
            } else if frame_refs().count(paddr).is_none() {
                return Err(AxError::BadAddress);
            }

            match pt.unmap(vaddr) {
                Ok((unmapped, _unmapped_flags, unmapped_size)) => {
                    if unmapped != paddr || unmapped_size != self.size {
                        // Keep every physical-page reference alive on an
                        // impossible metadata/PTE disagreement. Reinstalling
                        // either side would publish a mapping whose ownership
                        // record is unknown. The wrapper still flushes the PTE
                        // that `unmap` cleared before returning this error.
                        if let Some(page) = file_page {
                            file_page_refs
                                .expect("file page reference table disappeared")
                                .insert(vaddr, page);
                        }
                        return Err(AxError::BadAddress);
                    }
                }
                Err(_) => {
                    if let Some(page) = file_page {
                        file_page_refs
                            .expect("file page reference table disappeared")
                            .insert(vaddr, page);
                    }
                    return Err(AxError::BadAddress);
                }
            }

            if let Some(page) = file_page {
                gather.push_file(page, pt);
            } else {
                // The removed PTE's reference remains in the frame reference
                // table until the queued TLB invalidation has completed.
                gather.push_anonymous(paddr, self.size, pt);
            }
        }

        Ok(())
    }

    pub(crate) fn prepare_fault(
        &self,
        vaddr: VirtAddr,
        area_range: VirtAddrRange,
        _flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> FaultPreparation {
        if self.size != PageSize::Size4K {
            return FaultPreparation::Fallback;
        }
        let vaddr = vaddr.align_down(self.size as usize);
        match pt.query(vaddr) {
            Ok((paddr, page_flags, page_size)) => {
                if page_size == self.size && page_flags.contains(access_flags) {
                    FaultPreparation::Handled
                } else if page_size == self.size
                    && access_flags.contains(MappingFlags::WRITE)
                    && !page_flags.contains(MappingFlags::WRITE)
                {
                    let file_page = self.file_pages.as_ref().and_then(|pages| pages.get(vaddr));
                    let source = if let Some(page) = file_page {
                        if page.paddr() != paddr {
                            return FaultPreparation::Fallback;
                        }
                        CowFaultSource::File(page)
                    } else {
                        let Some(count) = frame_refs().reference_frame(paddr) else {
                            return FaultPreparation::Fallback;
                        };
                        if count == 2 {
                            CowFaultSource::Exclusive(paddr)
                        } else {
                            CowFaultSource::Anonymous(paddr)
                        }
                    };
                    FaultPreparation::Cow(CowFaultPlan {
                        identity: self.identity.clone(),
                        vaddr,
                        mapping_start: self.start,
                        area_range,
                        size: self.size,
                        file: None,
                        access_flags,
                        frame: None,
                        file_pages: Vec::new(),
                        source: Some(source),
                    })
                } else {
                    FaultPreparation::Fallback
                }
            }
            Err(PagingError::NotMapped) => {
                #[cfg(feature = "fault-repro-trace")]
                if crate::mm::is_page_fault_repro_address(vaddr) {
                    warn!(
                        "[PFTRACE 06] prepare-fault vma_backend=Cow pte=NotMapped backing={} \
                         access={access_flags:?} resolution=missing-page-plan",
                        if self.file.is_some() {
                            "file"
                        } else {
                            "anonymous"
                        },
                    );
                }
                FaultPreparation::Cow(CowFaultPlan {
                    identity: self.identity.clone(),
                    vaddr,
                    mapping_start: self.start,
                    area_range,
                    size: self.size,
                    file: self.file.clone(),
                    access_flags,
                    frame: None,
                    file_pages: Vec::new(),
                    source: None,
                })
            }
            Err(_) => FaultPreparation::Fallback,
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
        if !Arc::ptr_eq(&self.identity, &plan.identity) || self.size != plan.size {
            return Ok(FaultCommit::Stale);
        }
        if plan.source.is_none() {
            return self.commit_missing_fault(plan, area_range, flags, pt);
        }

        match pt.query(plan.vaddr) {
            Ok((paddr, page_flags, page_size)) => {
                if page_size == self.size && page_flags.contains(access_flags) {
                    Ok(FaultCommit::Handled)
                } else if let Some(source) = plan.source.as_ref() {
                    if page_size != self.size
                        || paddr != source.paddr()
                        || !access_flags.contains(MappingFlags::WRITE)
                        || page_flags.contains(MappingFlags::WRITE)
                    {
                        return Ok(FaultCommit::Stale);
                    }

                    match source {
                        CowFaultSource::Exclusive(paddr) => {
                            let Some(commit) = frame_refs().with_count(*paddr, |count| {
                                if count != 2 {
                                    return Ok(FaultCommit::Stale);
                                }
                                // One reference belongs to the PTE and one is
                                // the temporary pin held by this plan. The
                                // locked count keeps fork from adding another
                                // reference before the permission upgrade.
                                if !pt.remap_4k_concurrent_if(plan.vaddr, *paddr, *paddr, flags)? {
                                    return Ok(FaultCommit::Stale);
                                }
                                Ok(FaultCommit::Handled)
                            }) else {
                                return Ok(FaultCommit::Stale);
                            };
                            return commit;
                        }
                        CowFaultSource::File(_) => {
                            let frame = plan.frame.ok_or(AxError::BadAddress)?;
                            let Some(file_page_refs) = self.file_pages.as_ref() else {
                                return Ok(FaultCommit::Stale);
                            };
                            let mut file_pages = file_page_refs.shard(plan.vaddr).lock();
                            let Some(mapped_page) = file_pages.get(&plan.vaddr) else {
                                return Ok(FaultCommit::Stale);
                            };
                            if mapped_page.paddr() != paddr {
                                return Ok(FaultCommit::Stale);
                            }
                            if !pt.remap_4k_concurrent_if(plan.vaddr, paddr, frame, flags)? {
                                return Ok(FaultCommit::Stale);
                            }
                            file_pages.remove(&plan.vaddr);
                        }
                        CowFaultSource::Anonymous(_) => {
                            let frame = plan.frame.ok_or(AxError::BadAddress)?;
                            if !pt.remap_4k_concurrent_if(plan.vaddr, paddr, frame, flags)? {
                                return Ok(FaultCommit::Stale);
                            }
                            // Drop the reference formerly owned by this PTE;
                            // the plan's extra reference keeps the source alive
                            // until the unlocked copy is completely committed.
                            drop_referenced_frame(paddr, self.size);
                        }
                    }
                    if !matches!(source, CowFaultSource::Exclusive(_)) {
                        plan.frame = None;
                    }
                    Ok(FaultCommit::Handled)
                } else {
                    Ok(FaultCommit::Stale)
                }
            }
            Err(PagingError::NotMapped) => Ok(FaultCommit::Stale),
            Err(_) => Err(AxError::BadAddress),
        }
    }

    fn alloc_new_at(
        &self,
        vaddr: VirtAddr,
        flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult {
        let frame = self.alloc_new_frame(true)?;

        if let Some((file, file_start, file_end)) = &self.file {
            let buf = unsafe {
                slice::from_raw_parts_mut(phys_to_virt(frame).as_mut_ptr(), self.size as _)
            };
            // vaddr can be smaller than self.start (at most 1 page) due to
            // non-aligned mappings, we need to keep the gap clean.
            let start = self.start.as_usize().saturating_sub(vaddr.as_usize());
            assert!(start < self.size as _);

            let file_start =
                *file_start + vaddr.as_usize().saturating_sub(self.start.as_usize()) as u64;
            let max_read = file_end
                .map_or(u64::MAX, |end| end.saturating_sub(file_start))
                .min((buf.len() - start) as u64) as usize;

            if let Err(err) = file.read_at(&mut &mut buf[start..start + max_read], file_start) {
                self.drop_new_frame(frame);
                return Err(err);
            }
        }
        if let Err(err) = pt.map(vaddr, frame, self.size, flags) {
            self.drop_new_frame(frame);
            return Err(err.into());
        }
        self.mark_mapped();
        Ok(())
    }

    fn share_existing_at(
        &self,
        vaddr: VirtAddr,
        paddr: PhysAddr,
        flags: MappingFlags,
        old_pt: &mut PageTableMut,
        new_pt: &mut PageTableMut,
    ) -> AxResult {
        if frame_refs().reference_frame(paddr).is_none() {
            warn!("failed to reserve frame reference");
            return Err(AxError::BadAddress);
        }

        // Reserve the child's reference before publishing its PTE. This keeps
        // the frame alive without holding the shard lock across page-table
        // operations. Roll the reservation back if either operation fails.
        if let Err(err) = old_pt.protect(vaddr, flags) {
            drop_referenced_frame(paddr, self.size);
            return Err(err.into());
        }
        if let Err(err) = new_pt.map(vaddr, paddr, self.size, flags) {
            drop_referenced_frame(paddr, self.size);
            return Err(err.into());
        }
        Ok(())
    }

    fn rollback_shared_range(&self, range: VirtAddrRange, pt: &mut PageTableMut) {
        if let Err(err) = self.unmap_present(range, pt) {
            warn!("rollback COW clone failed to unmap {range:?}: {err:?}");
        }
    }

    fn handle_cow_fault(
        &self,
        vaddr: VirtAddr,
        paddr: PhysAddr,
        flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult {
        // End the map guard before allocating or changing the page table.  In
        // particular, the error paths below must be able to put the pin back
        // without recursively locking `file_pages`.
        let file_page_refs = self.file_pages.as_deref();
        let file_page = file_page_refs.and_then(|pages| pages.remove(vaddr));
        if let Some(page) = file_page {
            if page.paddr() != paddr {
                file_page_refs
                    .expect("file page reference table disappeared")
                    .insert(vaddr, page);
                return Err(AxError::BadAddress);
            }

            let new_frame = match self.alloc_new_frame(false) {
                Ok(frame) => frame,
                Err(err) => {
                    file_page_refs
                        .expect("file page reference table disappeared")
                        .insert(vaddr, page);
                    return Err(err);
                }
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(paddr).as_ptr(),
                    phys_to_virt(new_frame).as_mut_ptr(),
                    self.size as _,
                );
            }
            if let Err(err) = pt.remap(vaddr, new_frame, flags) {
                self.drop_new_frame(new_frame);
                file_page_refs
                    .expect("file page reference table disappeared")
                    .insert(vaddr, page);
                return Err(err.into());
            }
            pt.finish();
            drop(page);
            return Ok(());
        }

        let exclusive = frame_refs()
            .with_count(paddr, |count| -> AxResult<bool> {
                if count != 1 {
                    return Ok(false);
                }
                // The shard lock serializes this check with fork's reference
                // reservation: a frame may be made writable only while it is
                // still exclusive.
                pt.protect(vaddr, flags)?;
                Ok(true)
            })
            .ok_or(AxError::BadAddress)??;
        if exclusive {
            return Ok(());
        }

        // Multiple references: copy without holding the frame-table shard lock.
        let new_frame = self.alloc_new_frame(false)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                phys_to_virt(paddr).as_ptr(),
                phys_to_virt(new_frame).as_mut_ptr(),
                self.size as _,
            );
        }
        if let Err(err) = pt.remap(vaddr, new_frame, flags) {
            self.drop_new_frame(new_frame);
            return Err(err.into());
        }
        pt.finish();
        drop_referenced_frame(paddr, self.size);

        Ok(())
    }
}

impl BackendOps for CowBackend {
    fn page_size(&self) -> PageSize {
        self.size
    }

    fn map(&self, range: VirtAddrRange, flags: MappingFlags, _pt: &mut PageTableMut) -> AxResult {
        debug!("Cow::map: {range:?} {flags:?}",);
        Ok(())
    }

    fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        debug!("Cow::unmap: {range:?}");
        self.unmap_present(range, pt)
    }

    fn discard(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        self.unmap(range, pt)
    }

    fn populate(
        &self,
        range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult<(usize, Option<PopulateCallback>)> {
        let mut pages = 0;
        for addr in pages_in(range, self.size)? {
            match pt.query(addr) {
                Ok((paddr, page_flags, page_size)) => {
                    assert_eq!(self.size, page_size);
                    if access_flags.contains(MappingFlags::WRITE)
                        && !page_flags.contains(MappingFlags::WRITE)
                    {
                        self.handle_cow_fault(addr, paddr, flags, pt)?;
                        pages += 1;
                    } else if page_flags.contains(access_flags) {
                        pages += 1;
                    }
                }
                // If the page is not mapped, try map it.
                Err(PagingError::NotMapped) => {
                    self.alloc_new_at(addr, flags, pt)?;
                    pages += 1;
                }
                Err(_) => return Err(AxError::BadAddress),
            }
        }
        Ok((pages, None))
    }

    fn clone_map(
        &self,
        range: VirtAddrRange,
        flags: MappingFlags,
        old_pt: &mut PageTableMut,
        new_pt: &mut PageTableMut,
        _new_aspace: &Arc<AddrSpaceLock>,
    ) -> AxResult<Backend> {
        let new_backend = self.clone_for_fork();
        // Forking untouched lazy COW regions is common (e.g. fork14). In that
        // case, there is nothing in page tables to duplicate.
        if !self.has_mappings.load(Ordering::Relaxed) {
            return Ok(Backend::Cow(new_backend));
        }

        let cow_flags = flags - MappingFlags::WRITE;

        let mut cursor = range.start;
        while let Some((vaddr, paddr, page_flags, page_size)) =
            old_pt.next_present_mapping(cursor, range.end)
        {
            if vaddr < range.start || vaddr >= range.end || page_size != self.size {
                new_backend.rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                return Err(AxError::BadAddress);
            }
            cursor = vaddr + page_size as usize;

            // A clean page mapped directly from the file folio can be rebuilt
            // by the child's first fault. Do not copy its PTE or pin here.
            if let Some(page) = self.file_pages.as_ref().and_then(|pages| pages.get(vaddr)) {
                if page.paddr() != paddr || page_flags.contains(MappingFlags::WRITE) {
                    new_backend
                        .rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                    return Err(AxError::BadAddress);
                }
                continue;
            }

            if let Err(err) = self.share_existing_at(vaddr, paddr, cow_flags, old_pt, new_pt) {
                new_backend.rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                return Err(err);
            }
            new_backend.mark_mapped();
        }

        Ok(Backend::Cow(new_backend))
    }
}

impl Backend {
    pub fn new_cow(
        start: VirtAddr,
        size: PageSize,
        file: FileBackend,
        file_start: u64,
        file_end: Option<u64>,
    ) -> Self {
        Self::Cow(CowBackend {
            start,
            size,
            file: Some((file, file_start, file_end)),
            has_mappings: Arc::new(AtomicBool::new(false)),
            identity: Arc::new(()),
            file_pages: Some(Arc::new(FilePageRefs::new())),
        })
    }

    pub fn new_alloc(start: VirtAddr, size: PageSize) -> Self {
        Self::Cow(CowBackend {
            start,
            size,
            file: None,
            has_mappings: Arc::new(AtomicBool::new(false)),
            identity: Arc::new(()),
            file_pages: None,
        })
    }
}
