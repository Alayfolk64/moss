use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    slice,
    sync::atomic::{AtomicUsize, Ordering},
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
    UnmapGather, alloc_frame, dealloc_frame, finish_unmap_before_reclaim, pages_in,
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
/// Consecutive logical pages are striped across independently locked vectors,
/// preserving parallel access without a tree lookup and `Arc` per frame.
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

    fn count(&self, paddr: PhysAddr) -> Option<usize> {
        self.with_count(paddr, core::convert::identity)
    }

    /// Calls `f` while the frame's shard remains locked.
    ///
    /// A stable count is required when an exclusive COW mapping is upgraded:
    /// fork must not add a reference between the count check and PTE update.
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

struct FilePageRefs {
    shards: [Mutex<BTreeMap<VirtAddr, FolioPageRef>>; FILE_PAGE_SHARDS],
}

impl FilePageRefs {
    fn new() -> Self {
        Self {
            shards: [const { Mutex::new(BTreeMap::new()) }; FILE_PAGE_SHARDS],
        }
    }

    fn shard(&self, vaddr: VirtAddr) -> &Mutex<BTreeMap<VirtAddr, FolioPageRef>> {
        let page = vaddr.as_usize() / memory_addr::PAGE_SIZE_4K;
        &self.shards[page % FILE_PAGE_SHARDS]
    }

    fn get(&self, vaddr: VirtAddr) -> Option<FolioPageRef> {
        self.shard(vaddr).lock().get(&vaddr).cloned()
    }

    fn insert(&self, vaddr: VirtAddr, page: FolioPageRef) {
        let old = self.shard(vaddr).lock().insert(vaddr, page);
        assert!(old.is_none(), "replacing an existing file-page pin");
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
    mapped_pages: AtomicUsize,
    identity: Arc<()>,
    /// Directly mapped file pages need an owning reference for as long as
    /// their PTEs exist. Clones used to split one VMA share this table.
    file_pages: Arc<FilePageRefs>,
}

impl Clone for CowBackend {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            mapped_pages: AtomicUsize::new(self.mapped_pages.load(Ordering::Relaxed)),
            identity: self.identity.clone(),
            file_pages: self.file_pages.clone(),
        }
    }
}

enum CowFaultSource {
    /// An extra reference pins an anonymous COW source while the address-space
    /// lock is released.
    Anonymous(PhysAddr),
    /// A cloned folio reference pins a directly mapped file page.
    File(FolioPageRef),
}

impl CowFaultSource {
    fn paddr(&self) -> PhysAddr {
        match self {
            Self::Anonymous(paddr) => *paddr,
            Self::File(page) => page.paddr(),
        }
    }
}

/// A private fault prepared outside the address-space lock.
pub(crate) struct CowFaultPlan {
    identity: Arc<()>,
    vaddr: VirtAddr,
    mapping_start: VirtAddr,
    size: PageSize,
    file: Option<(FileBackend, u64, Option<u64>)>,
    access_flags: MappingFlags,
    frame: Option<PhysAddr>,
    file_page: Option<FolioPageRef>,
    source: Option<CowFaultSource>,
}

impl CowFaultPlan {
    pub(crate) fn execute(&mut self) -> AxResult {
        if let Some(source) = &self.source {
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
                .min((self.size as usize - start) as u64) as usize;

            // A complete aligned file page can stay in the folio cache and be
            // installed read-only. A later write fault performs the actual
            // private copy. Keep this stage deliberately single-page; folio
            // fault-around is a separate optimization.
            if self.size == PageSize::Size4K
                && !self.access_flags.contains(MappingFlags::WRITE)
                && start == 0
                && max_read == self.size as usize
                && let Some(page) = file.page_ref_at(fault_file_start)?
            {
                self.file_page = Some(page);
                return Ok(());
            }

            let frame = alloc_referenced_frame(true, self.size)?;
            self.frame = Some(frame);
            let buf = unsafe {
                slice::from_raw_parts_mut(phys_to_virt(frame).as_mut_ptr(), self.size as _)
            };
            file.read_at(&mut &mut buf[start..start + max_read], fault_file_start)?;
        } else {
            self.frame = Some(alloc_referenced_frame(true, self.size)?);
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
                CowFaultSource::Anonymous(frame) => drop_referenced_frame(frame, self.size),
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
    if frame_refs().release_frame(frame) {
        dealloc_frame(frame, size);
    }
}

impl CowBackend {
    pub(super) fn file_backed(&self) -> bool {
        self.file.is_some()
    }

    pub(super) fn merge_compatible(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }

    fn clone_for_fork(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            mapped_pages: AtomicUsize::new(0),
            identity: Arc::new(()),
            file_pages: Arc::new(FilePageRefs::new()),
        }
    }

    fn dec_mapped_pages(&self) {
        let mut old = self.mapped_pages.load(Ordering::Relaxed);
        while old != 0 {
            match self.mapped_pages.compare_exchange_weak(
                old,
                old - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(v) => old = v,
            }
        }
    }

    fn alloc_new_frame(&self, zeroed: bool) -> AxResult<PhysAddr> {
        alloc_referenced_frame(zeroed, self.size)
    }

    fn drop_new_frame(&self, frame: PhysAddr) {
        drop_referenced_frame(frame, self.size);
    }

    fn unmap_present(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        let mut gather = UnmapGather::new();
        let result = self.unmap_present_into(range, pt, &mut gather);
        // Errors may be returned after earlier PTEs were cleared. Keep those
        // pages alive until their queued local and remote invalidations finish.
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
        while let Some((vaddr, paddr, _flags, page_size)) =
            pt.next_present_mapping(cursor, range.end)
        {
            if vaddr < range.start || vaddr >= range.end || page_size != self.size {
                return Err(AxError::BadAddress);
            }
            cursor = vaddr + page_size as usize;

            // Resolve the owner before clearing the PTE. On failure, leave the
            // mapping and its reference intact.
            let file_page = self.file_pages.remove(vaddr);
            if let Some(page) = file_page.as_ref() {
                if page.paddr() != paddr {
                    self.file_pages.insert(vaddr, page.clone());
                    return Err(AxError::BadAddress);
                }
            } else if frame_refs().count(paddr).is_none() {
                return Err(AxError::BadAddress);
            }

            match pt.unmap(vaddr) {
                Ok((unmapped, _, unmapped_size))
                    if unmapped == paddr && unmapped_size == self.size => {}
                Ok(_) => {
                    // The PTE was cleared, but retaining this owner is safer
                    // than exposing a physical page without ownership data.
                    if let Some(page) = file_page {
                        self.file_pages.insert(vaddr, page);
                    }
                    return Err(AxError::BadAddress);
                }
                Err(err) => {
                    if let Some(page) = file_page {
                        self.file_pages.insert(vaddr, page);
                    }
                    return Err(err.into());
                }
            }

            if let Some(page) = file_page {
                gather.push_file(page, pt);
            } else {
                gather.push_anonymous(paddr, self.size, pt);
            }
            self.dec_mapped_pages();
        }
        Ok(())
    }

    pub(crate) fn prepare_fault(
        &self,
        vaddr: VirtAddr,
        _area_range: VirtAddrRange,
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
                    let source = if let Some(page) = self.file_pages.get(vaddr) {
                        if page.paddr() != paddr {
                            return FaultPreparation::Fallback;
                        }
                        CowFaultSource::File(page)
                    } else {
                        let Some(count) = frame_refs().reference_frame(paddr) else {
                            return FaultPreparation::Fallback;
                        };
                        // A new count of two means the PTE was the only owner.
                        // Undo the temporary pin and let the locked fallback
                        // path upgrade permissions without copying.
                        if count == 2 {
                            drop_referenced_frame(paddr, self.size);
                            return FaultPreparation::Fallback;
                        }
                        CowFaultSource::Anonymous(paddr)
                    };
                    FaultPreparation::Cow(CowFaultPlan {
                        identity: self.identity.clone(),
                        vaddr,
                        mapping_start: self.start,
                        size: self.size,
                        file: None,
                        access_flags,
                        frame: None,
                        file_page: None,
                        source: Some(source),
                    })
                } else {
                    FaultPreparation::Fallback
                }
            }
            Err(PagingError::NotMapped) => FaultPreparation::Cow(CowFaultPlan {
                identity: self.identity.clone(),
                vaddr,
                mapping_start: self.start,
                size: self.size,
                file: self.file.clone(),
                access_flags,
                frame: None,
                file_page: None,
                source: None,
            }),
            Err(_) => FaultPreparation::Fallback,
        }
    }

    pub(crate) fn commit_fault(
        &self,
        plan: &mut CowFaultPlan,
        _area_range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> AxResult<FaultCommit> {
        if !Arc::ptr_eq(&self.identity, &plan.identity) || self.size != plan.size {
            return Ok(FaultCommit::Stale);
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

                    let frame = plan.frame.ok_or(AxError::BadAddress)?;
                    let file_source = matches!(source, CowFaultSource::File(_));
                    let mapped_file_page = if file_source {
                        let mut file_pages = self.file_pages.shard(plan.vaddr).lock();
                        let Some(mapped_page) = file_pages.get(&plan.vaddr) else {
                            return Ok(FaultCommit::Stale);
                        };
                        if mapped_page.paddr() != paddr {
                            return Ok(FaultCommit::Stale);
                        }
                        if !pt.remap_4k_concurrent_if(plan.vaddr, paddr, frame, flags)? {
                            return Ok(FaultCommit::Stale);
                        }
                        file_pages.remove(&plan.vaddr)
                    } else {
                        if !pt.remap_4k_concurrent_if(plan.vaddr, paddr, frame, flags)? {
                            return Ok(FaultCommit::Stale);
                        }
                        None
                    };
                    // The leaf CAS replaced the physical frame. Before the
                    // old PTE reference can be released, every RISC-V hart
                    // must stop using a possibly cached translation to it.
                    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                    crate::mm::flush_tlb_on_all_cpus(pt.root_paddr(), Some(plan.vaddr));
                    if file_source {
                        drop(mapped_file_page.expect("validated file-page pin disappeared"));
                    } else {
                        // Drop the reference formerly owned by this PTE. The
                        // plan's extra pin remains until CowFaultPlan is dropped.
                        drop_referenced_frame(paddr, self.size);
                    }
                    plan.frame = None;
                    Ok(FaultCommit::Handled)
                } else {
                    Ok(FaultCommit::Stale)
                }
            }
            Err(PagingError::NotMapped) => {
                if plan.source.is_some() {
                    return Ok(FaultCommit::Stale);
                }
                if let Some(page) = plan.file_page.take() {
                    let Some((file, mapping_file_start, _)) = plan.file.as_ref() else {
                        return Ok(FaultCommit::Stale);
                    };
                    let file_offset = *mapping_file_start
                        + plan
                            .vaddr
                            .as_usize()
                            .saturating_sub(plan.mapping_start.as_usize())
                            as u64;
                    let mapping_page = page.clone();
                    let mapped = file.with_valid_page_ref(file_offset, &page, || {
                        let mut file_pages = self.file_pages.shard(plan.vaddr).lock();
                        match pt.map_4k_concurrent(
                            plan.vaddr,
                            page.paddr(),
                            flags - MappingFlags::WRITE,
                        ) {
                            Ok(()) => {
                                let old = file_pages.insert(plan.vaddr, mapping_page);
                                assert!(old.is_none(), "publishing duplicate file-page pin");
                                Ok(true)
                            }
                            Err(PagingError::AlreadyMapped) => Ok(false),
                            Err(err) => Err(err),
                        }
                    });
                    return match mapped {
                        Some(Ok(true)) => {
                            self.mapped_pages.fetch_add(1, Ordering::Relaxed);
                            Ok(FaultCommit::Handled)
                        }
                        Some(Ok(false)) | None => Ok(FaultCommit::Stale),
                        Some(Err(err)) => Err(err.into()),
                    };
                }
                let frame = plan.frame.ok_or(AxError::BadAddress)?;
                match pt.map_4k_concurrent(plan.vaddr, frame, flags) {
                    Ok(()) => plan.frame = None,
                    Err(PagingError::AlreadyMapped) => return Ok(FaultCommit::Stale),
                    Err(err) => return Err(err.into()),
                }
                self.mapped_pages.fetch_add(1, Ordering::Relaxed);
                Ok(FaultCommit::Handled)
            }
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
        self.mapped_pages.fetch_add(1, Ordering::Relaxed);
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

        // Reserve the child's reference before publishing its PTE. Roll the
        // reservation back if either page-table operation fails.
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
        let Ok(pages) = pages_in(range, self.size) else {
            return;
        };

        for vaddr in pages {
            match pt.unmap(vaddr) {
                Ok((frame, _flags, page_size)) => {
                    finish_unmap_before_reclaim(pt);
                    let file_page = self.file_pages.remove(vaddr);
                    if page_size != self.size {
                        warn!("rollback COW clone got unexpected page size: {page_size:?}");
                        continue;
                    }

                    if let Some(page) = file_page {
                        if page.paddr() != frame {
                            warn!("rollback COW clone got mismatched cached frame");
                        }
                        self.dec_mapped_pages();
                        continue;
                    }

                    if frame_refs().count(frame).is_none() {
                        warn!("rollback COW clone found unreferenced frame: {frame:?}");
                        continue;
                    }
                    drop_referenced_frame(frame, self.size);
                    self.dec_mapped_pages();
                }
                Err(PagingError::NotMapped) => {}
                Err(err) => warn!("rollback COW clone failed to unmap {vaddr:?}: {err:?}"),
            }
        }
    }

    fn handle_cow_fault(
        &self,
        vaddr: VirtAddr,
        paddr: PhysAddr,
        flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult {
        if let Some(page) = self.file_pages.remove(vaddr) {
            if page.paddr() != paddr {
                self.file_pages.insert(vaddr, page);
                return Err(AxError::BadAddress);
            }

            let new_frame = match self.alloc_new_frame(false) {
                Ok(frame) => frame,
                Err(err) => {
                    self.file_pages.insert(vaddr, page);
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
                self.file_pages.insert(vaddr, page);
                return Err(err.into());
            }
            finish_unmap_before_reclaim(pt);
            drop(page);
            return Ok(());
        }

        let exclusive = frame_refs()
            .with_count(paddr, |count| -> AxResult<bool> {
                if count != 1 {
                    return Ok(false);
                }
                // Serialize the exclusivity check with fork's reference
                // reservation before making this PTE writable.
                pt.protect(vaddr, flags)?;
                Ok(true)
            })
            .ok_or(AxError::BadAddress)??;
        if exclusive {
            return Ok(());
        }

        // The current PTE keeps the shared frame alive while it is copied.
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
        finish_unmap_before_reclaim(pt);
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
        if self.mapped_pages.load(Ordering::Relaxed) == 0 {
            return Ok(Backend::Cow(new_backend));
        }

        let cow_flags = flags - MappingFlags::WRITE;

        for vaddr in pages_in(range, self.size)? {
            // Copy data from old memory area to new memory area.
            match old_pt.query(vaddr) {
                Ok((paddr, _, page_size)) => {
                    assert_eq!(page_size, self.size);
                    // If the page is mapped in the old page table:
                    // - Update its permissions in the old page table using `flags`.
                    // - Map the same physical page into the new page table at the same
                    // virtual address, with the same page size and `flags`.
                    let file_page = self.file_pages.get(vaddr);
                    let result = if let Some(page) = file_page {
                        if page.paddr() != paddr {
                            Err(AxError::BadAddress)
                        } else {
                            (|| -> AxResult {
                                old_pt.protect(vaddr, cow_flags)?;
                                new_pt.map(vaddr, paddr, self.size, cow_flags)?;
                                new_backend.file_pages.insert(vaddr, page);
                                Ok(())
                            })()
                        }
                    } else {
                        self.share_existing_at(vaddr, paddr, cow_flags, old_pt, new_pt)
                    };
                    if let Err(err) = result {
                        new_backend
                            .rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                        return Err(err);
                    }
                    new_backend.mapped_pages.fetch_add(1, Ordering::Relaxed);
                }
                // If the page is not mapped, skip it.
                Err(PagingError::NotMapped) => {}
                Err(_) => {
                    new_backend
                        .rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                    return Err(AxError::BadAddress);
                }
            };
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
            mapped_pages: AtomicUsize::new(0),
            identity: Arc::new(()),
            file_pages: Arc::new(FilePageRefs::new()),
        })
    }

    pub fn new_alloc(start: VirtAddr, size: PageSize) -> Self {
        Self::Cow(CowBackend {
            start,
            size,
            file: None,
            mapped_pages: AtomicUsize::new(0),
            identity: Arc::new(()),
            file_pages: Arc::new(FilePageRefs::new()),
        })
    }
}
