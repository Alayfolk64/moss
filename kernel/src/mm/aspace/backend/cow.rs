use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    slice,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
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
const RECENT_PAGE_SHARDS: usize = 64;
const WORKING_SET_HOT_NANOS: u64 = 120 * 1_000_000_000;
const WORKING_SET_PROBE_NANOS: u64 = 5 * 1_000_000_000;
const WORKING_SET_SCAN_PAGES: usize = 8192;
const NO_FAULT_FILE_PAGE: u64 = u64::MAX;
const COW_MMAP_READAHEAD_PAGES: u64 = 32;

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

/// Compact anonymous-frame reference counts indexed by physical page.
/// Holes between RAM ranges consume no entries, and consecutive pages are
/// striped across independent shards.
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

    fn with_count<R>(&self, paddr: PhysAddr, f: impl FnOnce(usize) -> R) -> Option<R> {
        let location = self.location(paddr)?;
        let counts = self.shards[location.shard].lock();
        let count = counts[location.slot];
        (count != 0).then(|| f(count))
    }

    fn count(&self, paddr: PhysAddr) -> Option<usize> {
        self.with_count(paddr, core::convert::identity)
    }

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

    fn insert(&mut self, vaddr: VirtAddr, page: FolioPageRef) -> Option<FolioPageRef> {
        match self.inline.as_mut() {
            None => {
                debug_assert!(self.overflow.is_empty());
                self.inline = Some((vaddr, page));
                None
            }
            Some((inline_addr, inline_page)) if *inline_addr == vaddr => {
                Some(core::mem::replace(inline_page, page))
            }
            Some(_) => self.overflow.insert(vaddr, page),
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

struct SwappedPageRefs {
    pages: Mutex<BTreeMap<VirtAddr, usize>>,
}

/// Software working-set signal for LA264, whose PTE format has no accessed
/// bit. Newly faulted, copied or swapped-in anonymous pages get one bounded
/// generation of protection from proactive reclaim; an actual allocation
/// failure may still reclaim them.
struct RecentPageRefs {
    shards: [Mutex<BTreeMap<VirtAddr, u64>>; RECENT_PAGE_SHARDS],
}

impl RecentPageRefs {
    fn new() -> Self {
        Self {
            shards: [const { Mutex::new(BTreeMap::new()) }; RECENT_PAGE_SHARDS],
        }
    }

    fn shard(&self, vaddr: VirtAddr) -> &Mutex<BTreeMap<VirtAddr, u64>> {
        let page = vaddr.as_usize() / memory_addr::PAGE_SIZE_4K;
        &self.shards[page % RECENT_PAGE_SHARDS]
    }

    fn extend_until(&self, vaddr: VirtAddr, hot_until: u64) {
        let mut shard = self.shard(vaddr).lock();
        shard
            .entry(vaddr)
            .and_modify(|current| *current = (*current).max(hot_until))
            .or_insert(hot_until);
    }

    fn mark_active(&self, vaddr: VirtAddr) {
        self.extend_until(
            vaddr,
            axhal::time::monotonic_time_nanos().saturating_add(WORKING_SET_HOT_NANOS),
        );
    }

    fn mark_probe(&self, vaddr: VirtAddr, now: u64) {
        self.extend_until(vaddr, now.saturating_add(WORKING_SET_PROBE_NANOS));
    }

    /// A refault proves that the previous eviction selected a live working-set
    /// page. Keep it out of proactive reclaim for the rest of this process;
    /// an actual allocation failure still uses `allow_recent` and may reclaim
    /// it as the last resort.
    fn mark_refaulted(&self, vaddr: VirtAddr) {
        self.extend_until(vaddr, u64::MAX);
    }

    fn is_recent(&self, vaddr: VirtAddr, now: u64) -> bool {
        let mut shard = self.shard(vaddr).lock();
        match shard.get(&vaddr).copied() {
            Some(hot_until) if hot_until > now => true,
            Some(_) => {
                shard.remove(&vaddr);
                false
            }
            None => false,
        }
    }

    fn remove(&self, vaddr: VirtAddr) {
        self.shard(vaddr).lock().remove(&vaddr);
    }

    fn remove_range(&self, range: VirtAddrRange) {
        for shard in &self.shards {
            shard
                .lock()
                .retain(|vaddr, _| *vaddr < range.start || *vaddr >= range.end);
        }
    }
}

impl SwappedPageRefs {
    fn new() -> Self {
        Self {
            pages: Mutex::new(BTreeMap::new()),
        }
    }

    fn get(&self, vaddr: VirtAddr) -> Option<usize> {
        self.pages.lock().get(&vaddr).copied()
    }

    fn contains(&self, vaddr: VirtAddr) -> bool {
        self.pages.lock().contains_key(&vaddr)
    }

    fn insert(&self, vaddr: VirtAddr, slot: usize) {
        let old = self.pages.lock().insert(vaddr, slot);
        assert!(old.is_none(), "replacing an existing swapped COW page");
    }

    fn release_range(&self, range: VirtAddrRange) {
        self.pages.lock().retain(|vaddr, slot| {
            if *vaddr >= range.start && *vaddr < range.end {
                crate::swap::release_slot(*slot);
                false
            } else {
                true
            }
        });
    }

    fn clone_range_into(&self, range: VirtAddrRange, destination: &Self) -> AxResult {
        let pages = self.pages.lock();
        let mut destination = destination.pages.lock();
        for (vaddr, slot) in pages.range(range.start..range.end) {
            if !crate::swap::retain_slot(*slot) {
                return Err(AxError::Io);
            }
            let old = destination.insert(*vaddr, *slot);
            assert!(old.is_none(), "fork duplicated a swapped COW page");
        }
        Ok(())
    }
}

impl Drop for SwappedPageRefs {
    fn drop(&mut self) {
        for (_, slot) in core::mem::take(&mut *self.pages.lock()) {
            crate::swap::release_slot(slot);
        }
    }
}

struct CowReadaheadState {
    last_fault_file_page: AtomicU64,
    next_readahead_file_page: AtomicU64,
}

impl CowReadaheadState {
    const fn new() -> Self {
        Self {
            last_fault_file_page: AtomicU64::new(NO_FAULT_FILE_PAGE),
            next_readahead_file_page: AtomicU64::new(0),
        }
    }

    fn after_fault(&self, file: &FileBackend, file_offset: u64, file_end: Option<u64>) {
        let page_size = memory_addr::PAGE_SIZE_4K as u64;
        if !file_offset.is_multiple_of(page_size) {
            return;
        }
        let fault_page = file_offset / page_size;
        let previous = self
            .last_fault_file_page
            .swap(fault_page, Ordering::Relaxed);
        if previous.checked_add(1) != Some(fault_page) {
            return;
        }

        let start = fault_page.saturating_add(1);
        if start < self.next_readahead_file_page.load(Ordering::Relaxed) {
            return;
        }
        let mut end = start.saturating_add(COW_MMAP_READAHEAD_PAGES);
        if let Some(file_end) = file_end {
            end = end.min(file_end.div_ceil(page_size));
        }
        if start >= end {
            return;
        }

        self.next_readahead_file_page.store(end, Ordering::Relaxed);
        let count = usize::try_from((end - start) * page_size).unwrap_or(usize::MAX);
        let _ = file.readahead(start * page_size, count);
    }
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
    /// Set after the first resident PTE is installed.
    ///
    /// Fork only needs a conservative "definitely empty" fast path. Keeping
    /// an exact count makes unrelated faults and unmaps contend on one atomic
    /// cache line, while split VMAs also make a per-backend count inexact.
    /// Clones produced by VMA splitting therefore share this one-way flag.
    has_mappings: Arc<AtomicBool>,
    identity: Arc<()>,
    /// Directly mapped file pages need an owning reference for as long as
    /// their PTEs exist. Clones used to split one VMA share this table.
    file_pages: Option<Arc<FilePageRefs>>,
    /// Anonymous pages evicted from this private mapping. VMA splits share
    /// the map; fork creates a new map and increments each slot reference.
    swapped_pages: Arc<SwappedPageRefs>,
    recent_pages: Arc<RecentPageRefs>,
    swap_cursor: Arc<AtomicUsize>,
    /// Splits of one private mapping share a sequential-fault window. Forked
    /// address spaces start independent windows to avoid cross-process noise.
    readahead: Arc<CowReadaheadState>,
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
            swapped_pages: self.swapped_pages.clone(),
            recent_pages: self.recent_pages.clone(),
            swap_cursor: self.swap_cursor.clone(),
            readahead: self.readahead.clone(),
        }
    }
}

enum CowFaultSource {
    /// The PTE owns the only ordinary reference. The extra plan reference
    /// keeps the frame alive while the address-space lock is released.
    Exclusive(PhysAddr),
    /// An extra reference pins an anonymous COW source while the address-space
    /// lock is released.
    Anonymous(PhysAddr),
    /// A cloned folio reference pins a directly mapped file page.
    File(FolioPageRef),
}

#[derive(Clone, Copy, Debug)]
struct SwapCandidate {
    vaddr: VirtAddr,
    paddr: PhysAddr,
    flags: MappingFlags,
    slot: usize,
}

impl CowFaultSource {
    fn paddr(&self) -> PhysAddr {
        match self {
            Self::Exclusive(paddr) | Self::Anonymous(paddr) => *paddr,
            Self::File(page) => page.paddr(),
        }
    }
}

/// A private fault prepared outside the address-space lock.
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
    swap_slot: Option<usize>,
    readahead: Arc<CowReadaheadState>,
    source: Option<CowFaultSource>,
}

impl CowFaultPlan {
    pub(crate) fn execute(&mut self) -> AxResult {
        if let Some(slot) = self.swap_slot {
            let frame = alloc_referenced_frame(false, self.size)?;
            self.frame = Some(frame);
            crate::swap::read_page(slot, frame)?;
            return Ok(());
        }
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
                .min((self.size as usize - start) as u64) as usize;

            // A complete aligned file page can stay in the folio cache and be
            // installed read-only. Demand-load the faulting page's naturally
            // aligned folio, then prepare its ready siblings for fault-around.
            if self.size == PageSize::Size4K
                && !self.access_flags.contains(MappingFlags::WRITE)
                && start == 0
                && max_read == self.size as usize
            {
                self.readahead
                    .after_fault(file, fault_file_start, *file_end);
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

                    // Resolve the demanded address first; sibling mappings are
                    // optional and must never delay fault completion on error.
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
                CowFaultSource::Exclusive(frame) | CowFaultSource::Anonymous(frame) => {
                    drop_referenced_frame(frame, self.size)
                }
                CowFaultSource::File(_) => {}
            }
        }
        if let Some(slot) = self.swap_slot.take() {
            crate::swap::release_slot(slot);
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
            has_mappings: Arc::new(AtomicBool::new(false)),
            identity: Arc::new(()),
            file_pages: self
                .file_pages
                .as_ref()
                .map(|_| Arc::new(FilePageRefs::new())),
            swapped_pages: Arc::new(SwappedPageRefs::new()),
            recent_pages: Arc::new(RecentPageRefs::new()),
            swap_cursor: Arc::new(AtomicUsize::new(self.start.as_usize())),
            readahead: Arc::new(CowReadaheadState::new()),
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
        let file_page_refs = self.file_pages.as_deref();
        while let Some((vaddr, paddr, _flags, page_size)) =
            pt.next_present_mapping(cursor, range.end)
        {
            if vaddr < range.start || vaddr >= range.end || page_size != self.size {
                return Err(AxError::BadAddress);
            }
            cursor = vaddr + page_size as usize;

            // Resolve the owner before clearing the PTE. On failure, leave the
            // mapping and its reference intact.
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
                Ok((unmapped, _, unmapped_size))
                    if unmapped == paddr && unmapped_size == self.size => {}
                Ok(_) => {
                    // The PTE was cleared, but retaining this owner is safer
                    // than exposing a physical page without ownership data.
                    if let Some(page) = file_page {
                        file_page_refs
                            .expect("file page reference table disappeared")
                            .insert(vaddr, page);
                    }
                    return Err(AxError::BadAddress);
                }
                Err(err) => {
                    if let Some(page) = file_page {
                        file_page_refs
                            .expect("file page reference table disappeared")
                            .insert(vaddr, page);
                    }
                    return Err(err.into());
                }
            }

            if let Some(page) = file_page {
                gather.push_file(page, pt);
            } else {
                gather.push_anonymous(paddr, self.size, pt);
            }
        }
        self.swapped_pages.release_range(range);
        self.recent_pages.remove_range(range);
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
                        swap_slot: None,
                        readahead: self.readahead.clone(),
                        source: Some(source),
                    })
                } else {
                    FaultPreparation::Fallback
                }
            }
            Err(PagingError::NotMapped) => {
                let swap_slot = self.swapped_pages.get(vaddr).map(|slot| {
                    assert!(
                        crate::swap::retain_slot(slot),
                        "swapped COW page lost its slot ownership"
                    );
                    slot
                });
                FaultPreparation::Cow(CowFaultPlan {
                    identity: self.identity.clone(),
                    vaddr,
                    mapping_start: self.start,
                    area_range,
                    size: self.size,
                    file: if swap_slot.is_some() {
                        None
                    } else {
                        self.file.clone()
                    },
                    access_flags,
                    frame: None,
                    file_pages: Vec::new(),
                    swap_slot,
                    readahead: self.readahead.clone(),
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

                    if let CowFaultSource::Exclusive(source_paddr) = source {
                        let Some(commit) = frame_refs().with_count(*source_paddr, |count| {
                            if count != 2 {
                                return Ok(FaultCommit::Stale);
                            }
                            // One reference belongs to the PTE and one to the
                            // fault plan. Keep the count stable until the PTE
                            // permission upgrade has committed.
                            if !pt.remap_4k_concurrent_if(
                                plan.vaddr,
                                *source_paddr,
                                *source_paddr,
                                flags,
                            )? {
                                return Ok(FaultCommit::Stale);
                            }
                            Ok(FaultCommit::Handled)
                        }) else {
                            return Ok(FaultCommit::Stale);
                        };
                        return commit;
                    }

                    let frame = plan.frame.ok_or(AxError::BadAddress)?;
                    let file_source = matches!(source, CowFaultSource::File(_));
                    let mapped_file_page = if file_source {
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
                        file_pages.remove(&plan.vaddr)
                    } else {
                        if !pt.remap_4k_concurrent_if(plan.vaddr, paddr, frame, flags)? {
                            return Ok(FaultCommit::Stale);
                        }
                        None
                    };
                    if file_source {
                        drop(mapped_file_page.expect("validated file-page pin disappeared"));
                    } else {
                        // Drop the reference formerly owned by this PTE. The
                        // plan's extra pin remains until CowFaultPlan is dropped.
                        drop_referenced_frame(paddr, self.size);
                    }
                    plan.frame = None;
                    self.recent_pages.mark_active(plan.vaddr);
                    Ok(FaultCommit::Handled)
                } else {
                    Ok(FaultCommit::Stale)
                }
            }
            Err(PagingError::NotMapped) => {
                if plan.source.is_some() {
                    return Ok(FaultCommit::Stale);
                }
                if let Some(slot) = plan.swap_slot {
                    let frame = plan.frame.ok_or(AxError::BadAddress)?;
                    let mut swapped = self.swapped_pages.pages.lock();
                    if swapped.get(&plan.vaddr).copied() != Some(slot) {
                        return Ok(FaultCommit::Stale);
                    }
                    match pt.map_4k_concurrent(plan.vaddr, frame, flags) {
                        Ok(()) => {
                            swapped.remove(&plan.vaddr);
                        }
                        Err(PagingError::AlreadyMapped) => return Ok(FaultCommit::Stale),
                        Err(err) => return Err(err.into()),
                    }
                    drop(swapped);
                    // Drop the mapping's slot owner. The plan pin remains
                    // until CowFaultPlan is dropped after this commit.
                    crate::swap::release_slot(slot);
                    plan.frame = None;
                    crate::swap::note_working_set_refault();
                    self.recent_pages.mark_refaulted(plan.vaddr);
                    self.mark_mapped();
                    return Ok(FaultCommit::Handled);
                }
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
                            let mut file_pages = file_page_refs.shard(addr).lock();
                            match pt.map_4k_concurrent(
                                addr,
                                page.paddr(),
                                flags - MappingFlags::WRITE,
                            ) {
                                Ok(()) => {
                                    let old = file_pages.insert(addr, mapping_page);
                                    assert!(old.is_none(), "publishing duplicate file-page pin");
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
                    Ok(()) => plan.frame = None,
                    Err(PagingError::AlreadyMapped) => return Ok(FaultCommit::Stale),
                    Err(err) => return Err(err.into()),
                }
                self.recent_pages.mark_active(plan.vaddr);
                self.mark_mapped();
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
        self.recent_pages.mark_active(vaddr);
        self.mark_mapped();
        Ok(())
    }

    fn swap_in_at(
        &self,
        vaddr: VirtAddr,
        slot: usize,
        flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult {
        let frame = self.alloc_new_frame(false)?;
        if let Err(err) = crate::swap::read_page(slot, frame) {
            self.drop_new_frame(frame);
            return Err(err);
        }
        if let Err(err) = pt.map(vaddr, frame, self.size, flags) {
            self.drop_new_frame(frame);
            return Err(err.into());
        }
        let removed = self.swapped_pages.pages.lock().remove(&vaddr);
        if removed != Some(slot) {
            let _ = pt.unmap(vaddr);
            pt.finish();
            self.drop_new_frame(frame);
            return Err(AxError::BadAddress);
        }
        crate::swap::release_slot(slot);
        crate::swap::note_working_set_refault();
        self.recent_pages.mark_refaulted(vaddr);
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
        let file_page_refs = self.file_pages.as_deref();
        if let Some(page) = file_page_refs.and_then(|pages| pages.remove(vaddr)) {
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
            self.recent_pages.mark_active(vaddr);
            return Ok(());
        }

        let exclusive = frame_refs()
            .with_count(paddr, |count| -> AxResult<bool> {
                if count != 1 {
                    return Ok(false);
                }
                // Serialize the exclusivity check with fork's reference
                // reservation until the writable PTE is published.
                pt.protect(vaddr, flags)?;
                Ok(true)
            })
            .ok_or(AxError::BadAddress)??;
        if exclusive {
            self.recent_pages.mark_active(vaddr);
            return Ok(());
        }

        // Copy shared data without holding the frame-reference shard lock.
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
        self.recent_pages.mark_active(vaddr);
        Ok(())
    }

    pub(super) fn swap_out_pages(
        &self,
        range: VirtAddrRange,
        pt: &mut PageTableMut,
        mapping_flags: MappingFlags,
        limit: usize,
        allow_recent: bool,
    ) -> usize {
        const BATCH_PAGES: usize = 128;
        if self.size != PageSize::Size4K || limit == 0 || !crate::swap::enabled() {
            return 0;
        }

        let saved_cursor = VirtAddr::from_usize(self.swap_cursor.load(Ordering::Relaxed));
        let first = if saved_cursor >= range.start && saved_cursor < range.end {
            saved_cursor
        } else {
            range.start
        };
        let mut cursor = first;
        let mut wrapped = false;
        let mut inspected = 0usize;
        let mut sampled = 0usize;
        let mut candidates = heapless::Vec::<SwapCandidate, BATCH_PAGES>::new();
        let now = axhal::time::monotonic_time_nanos();

        while candidates.len() < limit.min(BATCH_PAGES) && inspected < WORKING_SET_SCAN_PAGES {
            if cursor >= range.end {
                if wrapped || first == range.start {
                    break;
                }
                cursor = range.start;
                wrapped = true;
            }
            if wrapped && cursor >= first {
                break;
            }
            let vaddr = cursor;
            cursor += memory_addr::PAGE_SIZE_4K;
            inspected += 1;

            if self
                .file_pages
                .as_ref()
                .is_some_and(|pages| pages.get(vaddr).is_some())
                || self.swapped_pages.contains(vaddr)
            {
                continue;
            }
            if !allow_recent && self.recent_pages.is_recent(vaddr, now) {
                continue;
            }
            let Ok((paddr, _flags, page_size)) = pt.query(vaddr) else {
                continue;
            };
            if page_size != PageSize::Size4K {
                continue;
            }
            if frame_refs().count(paddr) != Some(1) {
                // Shared fork pages and fault-plan pins cannot be evicted by
                // this first implementation; their ownership stays in RAM.
                continue;
            }
            // LA264 has no PTE accessed bit, but it does have a software-
            // visible dirty bit. The first pass clears D while preserving W;
            // a real write raises PageModify and the ordinary COW fault path
            // marks the page active. Only an entry that remains clean through
            // the grace period reaches eviction. Forced allocation-failure
            // reclaim bypasses sampling so it can always make progress.
            if !allow_recent && mapping_flags.contains(MappingFlags::WRITE) {
                match pt.clear_dirty(vaddr) {
                    Ok(Some(true)) => {
                        self.recent_pages.mark_probe(vaddr, now);
                        sampled += 1;
                        continue;
                    }
                    Ok(Some(false)) | Ok(None) => {}
                    Err(_) => continue,
                }
            }
            let Some(slot) = crate::swap::reserve_slot() else {
                break;
            };
            match pt.unmap(vaddr) {
                Ok((unmapped, _flags, PageSize::Size4K)) if unmapped == paddr => {
                    candidates
                        .push(SwapCandidate {
                            vaddr,
                            paddr,
                            // `flags` intentionally reports WRITE as absent
                            // while LoongArch D is clear. Preserve the VMA's
                            // real permissions for I/O-error rollback.
                            flags: mapping_flags,
                            slot,
                        })
                        .expect("bounded swap candidate batch");
                }
                Ok(_) | Err(_) => crate::swap::release_slot(slot),
            }
        }
        self.swap_cursor.store(
            if cursor < range.end {
                cursor.as_usize()
            } else {
                range.start.as_usize()
            },
            Ordering::Relaxed,
        );

        crate::swap::note_working_set_probes(sampled);
        if candidates.is_empty() {
            if sampled != 0 {
                // Clearing D is permission tightening. Flush both CPUs before
                // userspace can run and provide the PageModify signal.
                pt.finish();
            }
            return 0;
        }
        // No CPU may keep accessing a page while AHCI copies it to disk.
        pt.finish();

        let mut reclaimed = 0;
        for candidate in candidates {
            match crate::swap::write_page(candidate.slot, candidate.paddr) {
                Ok(()) => {
                    self.recent_pages.remove(candidate.vaddr);
                    self.swapped_pages.insert(candidate.vaddr, candidate.slot);
                    drop_referenced_frame(candidate.paddr, PageSize::Size4K);
                    reclaimed += 1;
                }
                Err(_) => {
                    // Keep the original page alive if disk I/O failed. Restore
                    // its PTE before returning to userspace.
                    if let Err(err) = pt.map(
                        candidate.vaddr,
                        candidate.paddr,
                        PageSize::Size4K,
                        candidate.flags,
                    ) {
                        error!(
                            "MOSS_SWAP failed to restore {:?} after I/O error: {:?}",
                            candidate.vaddr, err
                        );
                    }
                    crate::swap::release_slot(candidate.slot);
                }
            }
        }
        reclaimed
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
                    if let Some(slot) = self.swapped_pages.get(addr) {
                        self.swap_in_at(addr, slot, flags, pt)?;
                    } else {
                        self.alloc_new_at(addr, flags, pt)?;
                    }
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
        self.swapped_pages
            .clone_range_into(range, &new_backend.swapped_pages)?;
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

            // Keep a resident file-folio mapping resident across fork. The
            // ownership reference is already available, and inheriting it
            // avoids sending the child back through page-cache/I/O on first
            // access.
            if let Some(page) = self.file_pages.as_ref().and_then(|pages| pages.get(vaddr)) {
                if page.paddr() != paddr || page_flags.contains(MappingFlags::WRITE) {
                    new_backend
                        .rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                    return Err(AxError::BadAddress);
                }
                if let Err(err) = (|| -> AxResult {
                    old_pt.protect(vaddr, cow_flags)?;
                    new_pt.map(vaddr, paddr, self.size, cow_flags)?;
                    new_backend
                        .file_pages
                        .as_ref()
                        .expect("file-backed clone lost its ownership table")
                        .insert(vaddr, page);
                    Ok(())
                })() {
                    new_backend
                        .rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                    return Err(err);
                }
                new_backend.mark_mapped();
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
            swapped_pages: Arc::new(SwappedPageRefs::new()),
            recent_pages: Arc::new(RecentPageRefs::new()),
            swap_cursor: Arc::new(AtomicUsize::new(start.as_usize())),
            readahead: Arc::new(CowReadaheadState::new()),
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
            swapped_pages: Arc::new(SwappedPageRefs::new()),
            recent_pages: Arc::new(RecentPageRefs::new()),
            swap_cursor: Arc::new(AtomicUsize::new(start.as_usize())),
            readahead: Arc::new(CowReadaheadState::new()),
        })
    }
}
