use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};

use axerrno::{AxError, AxResult};
use axfs::{FileBackend, FolioPageRef};
use axhal::{
    mem::phys_to_virt,
    paging::{MappingFlags, PageSize, PageTable, PageTableMut, PagingError},
};
use axsync::Mutex;
use kspin::SpinNoIrq;
use memory_addr::{MemoryAddr, PhysAddr, VirtAddr, VirtAddrRange};

use super::{
    AddrSpaceLock, Backend, BackendOps, FaultCommit, FaultPreparation, PopulateCallback,
    alloc_frame, dealloc_frame, pages_in,
};

struct FrameRefCnt(usize);

/// Linux defaults `fault_around_bytes` to 64 KiB.  Use the same naturally
/// aligned window so sequential instruction/data access amortizes the trap,
/// address-space lock and page-table update over up to sixteen cached pages.
const FILE_FAULT_AROUND_BYTES: usize = 64 * 1024;

impl FrameRefCnt {
    // This function may lock FRAME_TABLE again, so the caller should drop the lock first.
    fn drop_frame(&mut self, paddr: PhysAddr, page_size: PageSize) {
        assert!(self.0 > 0, "dropping unreferenced frame");
        self.0 -= 1;
        if self.0 == 0 {
            // Remove the frame from FRAME_TABLE before deallocating it to avoid a race:
            // if we dealloc the frame first, another thread could allocate the same
            // physical frame before we remove the table entry. This function assumes
            // the caller is not holding the FRAME_TABLE lock, so it is safe to lock
            // FRAME_TABLE here and perform the removal.
            frame_table(paddr).lock().remove_frame(paddr);
            dealloc_frame(paddr, page_size);
        }
    }
}

struct FrameTableRefCount {
    table: BTreeMap<PhysAddr, Arc<SpinNoIrq<FrameRefCnt>>>,
}

impl FrameTableRefCount {
    const INITIAL_CNT: usize = 1;

    const fn new() -> Self {
        Self {
            table: BTreeMap::new(),
        }
    }

    fn get_frame_ref(&mut self, paddr: PhysAddr) -> Option<Arc<SpinNoIrq<FrameRefCnt>>> {
        self.table.get(&paddr).cloned()
    }

    fn init_frame(&mut self, paddr: PhysAddr) {
        assert!(
            !self.table.contains_key(&paddr),
            "initializing already referenced frame"
        );
        self.table.insert(
            paddr,
            Arc::new(SpinNoIrq::new(FrameRefCnt(Self::INITIAL_CNT))),
        );
    }

    fn remove_frame(&mut self, paddr: PhysAddr) {
        assert!(
            self.table.contains_key(&paddr),
            "removing unreferenced frame"
        );
        self.table.remove(&paddr);
    }
}

const FRAME_TABLE_SHARDS: usize = 64;
const FILE_PAGE_SHARDS: usize = 64;

static FRAME_TABLE: [SpinNoIrq<FrameTableRefCount>; FRAME_TABLE_SHARDS] =
    [const { SpinNoIrq::new(FrameTableRefCount::new()) }; FRAME_TABLE_SHARDS];

fn frame_table(paddr: PhysAddr) -> &'static SpinNoIrq<FrameTableRefCount> {
    let index = (paddr.as_usize() / memory_addr::PAGE_SIZE_4K) % FRAME_TABLE_SHARDS;
    &FRAME_TABLE[index]
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
    mapped_pages: Arc<AtomicUsize>,
    identity: Arc<()>,
    /// Cached file pages mapped directly into this address space. Keeping the
    /// reference here pins the frame even after it leaves the file-cache LRU.
    file_pages: Arc<FilePageRefs>,
}

impl Clone for CowBackend {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            mapped_pages: self.mapped_pages.clone(),
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
            // faulting page is demand-loaded while the remainder of the
            // naturally aligned 64-KiB window enters the page cache
            // asynchronously. Never cross the current VMA or file-backed
            // portion of the mapping.
            if self.size == PageSize::Size4K
                && !self.access_flags.contains(MappingFlags::WRITE)
                && start == 0
                && max_read == self.size as usize
            {
                let page_size = self.size as usize;
                // Align the fault-around window to the file folio rather than
                // to the randomized virtual address. ELF mappings are only
                // page-aligned, so virtual 64-KiB alignment can otherwise
                // straddle two file folios and leave most of the folio we just
                // loaded unmapped.
                let offset_in_folio = (fault_file_start % FILE_FAULT_AROUND_BYTES as u64) as usize;
                let window_start =
                    VirtAddr::from(self.vaddr.as_usize().saturating_sub(offset_in_folio));
                let window_end = window_start + FILE_FAULT_AROUND_BYTES;
                let first = window_start
                    .max(self.area_range.start.align_up(page_size))
                    .max(self.mapping_start.align_up(page_size));
                let last = window_end.min(self.area_range.end.align_down(page_size));

                // `page_ref_at` demand-loads the aligned folio containing the
                // fault. Once it completes, the following cache-only walk can
                // install neighboring pages without issuing more I/O.
                if let Some(page) = file.page_ref_at(fault_file_start)? {
                    // Put the faulting page first so commit either resolves the
                    // original fault or reports failure before optional pages.
                    self.file_pages.push((self.vaddr, page));
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
                            if complete && let Some(page) = file.cached_page_ref_at(offset) {
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
                    drop_referenced_frame(frame, self.size);
                }
                CowFaultSource::File(_) => {}
            }
        }
    }
}

fn alloc_referenced_frame(zeroed: bool, size: PageSize) -> AxResult<PhysAddr> {
    let frame = alloc_frame(zeroed, size)?;
    frame_table(frame).lock().init_frame(frame);
    Ok(frame)
}

fn drop_referenced_frame(frame: PhysAddr, size: PageSize) {
    let frame_ref = frame_table(frame)
        .lock()
        .get_frame_ref(frame)
        .expect("new frame must be referenced");
    frame_ref.lock().drop_frame(frame, size);
}

impl CowBackend {
    fn clone_for_fork(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            mapped_pages: Arc::new(AtomicUsize::new(0)),
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
                    let file_page = self.file_pages.get(vaddr);
                    let source = if let Some(page) = file_page {
                        if page.paddr() != paddr {
                            return FaultPreparation::Fallback;
                        }
                        CowFaultSource::File(page)
                    } else {
                        let Some(frame_ref) = frame_table(paddr).lock().get_frame_ref(paddr) else {
                            return FaultPreparation::Fallback;
                        };
                        let mut frame_ref = frame_ref.lock();
                        let Some(count) = frame_ref.0.checked_add(1) else {
                            return FaultPreparation::Fallback;
                        };
                        let source = if frame_ref.0 == 1 {
                            CowFaultSource::Exclusive(paddr)
                        } else {
                            CowFaultSource::Anonymous(paddr)
                        };
                        frame_ref.0 = count;
                        source
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
            Err(PagingError::NotMapped) => FaultPreparation::Cow(CowFaultPlan {
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
            }),
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

                    match source {
                        CowFaultSource::Exclusive(paddr) => {
                            let Some(frame_ref) = frame_table(*paddr).lock().get_frame_ref(*paddr)
                            else {
                                return Ok(FaultCommit::Stale);
                            };
                            let frame_ref = frame_ref.lock();
                            // One reference belongs to the PTE and one is the
                            // temporary pin held by this plan. A larger count
                            // means the page became shared while the lock was
                            // released and must be reconsidered as a real COW.
                            if frame_ref.0 != 2 {
                                return Ok(FaultCommit::Stale);
                            }
                            if !pt.remap_4k_concurrent_if(plan.vaddr, *paddr, *paddr, flags)? {
                                return Ok(FaultCommit::Stale);
                            }
                        }
                        CowFaultSource::File(_) => {
                            let frame = plan.frame.ok_or(AxError::BadAddress)?;
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
            Err(PagingError::NotMapped) => {
                if plan.source.is_some() {
                    return Ok(FaultCommit::Stale);
                }
                if !plan.file_pages.is_empty() {
                    let Some((file, mapping_file_start, _)) = plan.file.as_ref() else {
                        return Ok(FaultCommit::Stale);
                    };
                    let pages = core::mem::take(&mut plan.file_pages);
                    let mut installed = 0;
                    for (addr, page) in pages {
                        if !area_range.contains(addr) {
                            continue;
                        }
                        match pt.query(addr) {
                            Err(PagingError::NotMapped) => {
                                let offset = *mapping_file_start
                                    + addr
                                        .as_usize()
                                        .saturating_sub(plan.mapping_start.as_usize())
                                        as u64;
                                let mapping_page = page.clone();
                                let mapped = file.with_valid_page_ref(offset, &page, || {
                                    // Hold only this address shard while
                                    // publishing the PTE and its cache-page
                                    // pin. A COW fault on the same page cannot
                                    // observe a half-published mapping.
                                    let mut file_pages = self.file_pages.shard(addr).lock();
                                    match pt.map_4k_concurrent(
                                        addr,
                                        page.paddr(),
                                        flags - MappingFlags::WRITE,
                                    ) {
                                        Ok(()) => {
                                            file_pages.insert(addr, mapping_page);
                                            true
                                        }
                                        Err(_) => false,
                                    }
                                });
                                if mapped != Some(true) {
                                    if addr == plan.vaddr {
                                        return Ok(FaultCommit::Stale);
                                    }
                                    continue;
                                }
                                installed += 1;
                            }
                            Ok(_) => {}
                            Err(err) if addr == plan.vaddr => return Err(err.into()),
                            Err(_) => {}
                        }
                    }
                    self.mapped_pages.fetch_add(installed, Ordering::Relaxed);
                    return match pt.query(plan.vaddr) {
                        Ok((_paddr, page_flags, page_size))
                            if page_size == self.size && page_flags.contains(access_flags) =>
                        {
                            Ok(FaultCommit::Handled)
                        }
                        _ => Ok(FaultCommit::Stale),
                    };
                } else {
                    let frame = plan.frame.ok_or(AxError::BadAddress)?;
                    match pt.map_4k_concurrent(plan.vaddr, frame, flags) {
                        Ok(()) => plan.frame = None,
                        Err(PagingError::AlreadyMapped) => return Ok(FaultCommit::Stale),
                        Err(err) => return Err(err.into()),
                    }
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
        let frame = frame_table(paddr)
            .lock()
            .get_frame_ref(paddr)
            .ok_or(AxError::BadAddress)?;
        let mut frame = frame.lock();
        assert!(frame.0 > 0, "referencing unreferenced frame");
        let Some(new_ref_cnt) = frame.0.checked_add(1) else {
            warn!("frame reference count overflow");
            return Err(AxError::BadAddress);
        };
        old_pt.protect(vaddr, flags)?;
        new_pt.map(vaddr, paddr, self.size, flags)?;
        frame.0 = new_ref_cnt;
        Ok(())
    }

    fn rollback_shared_range(&self, range: VirtAddrRange, pt: &mut PageTableMut) {
        let Ok(pages) = pages_in(range, self.size) else {
            return;
        };

        for vaddr in pages {
            match pt.unmap(vaddr) {
                Ok((frame, _flags, page_size)) => {
                    let file_page = self.file_pages.remove(vaddr);
                    pt.finish();
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

                    let Some(frame_ref) = frame_table(frame).lock().get_frame_ref(frame) else {
                        warn!("rollback COW clone found unreferenced frame: {frame:?}");
                        continue;
                    };
                    frame_ref.lock().drop_frame(frame, self.size);
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
        // End the map guard before allocating or changing the page table.  In
        // particular, the error paths below must be able to put the pin back
        // without recursively locking `file_pages`.
        let file_page = self.file_pages.remove(vaddr);
        if let Some(page) = file_page {
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
            pt.finish();
            drop(page);
            return Ok(());
        }

        let frame_ref = frame_table(paddr)
            .lock()
            .get_frame_ref(paddr)
            .ok_or(AxError::BadAddress)?;
        let frame = frame_ref.lock();
        assert!(frame.0 > 0, "invalid frame reference count");
        match frame.0 {
            1 => {
                // Only one reference, just upgrade the permissions.
                pt.protect(vaddr, flags)?;
                return Ok(());
            }
            _ => {
                // Multiple references, need to copy the frame.
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
                drop(frame);
                pt.finish();
                frame_ref.lock().drop_frame(paddr, self.size);
            }
        }

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
        for addr in pages_in(range, self.size)? {
            if let Ok((frame, _flags, page_size)) = pt.unmap(addr) {
                assert_eq!(page_size, self.size);
                let file_page = self.file_pages.remove(addr);
                pt.finish();
                if let Some(page) = file_page {
                    if page.paddr() != frame {
                        warn!("unmapping COW page got mismatched cached frame");
                    }
                } else {
                    let frame_ref = frame_table(frame)
                        .lock()
                        .get_frame_ref(frame)
                        .ok_or(AxError::BadAddress)?;
                    let mut frame_ref = frame_ref.lock();
                    frame_ref.drop_frame(frame, self.size);
                }
                self.dec_mapped_pages();
            } else {
                // Deallocation is needn't if the page is not allocated.
            }
        }
        Ok(())
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
            mapped_pages: Arc::new(AtomicUsize::new(0)),
            identity: Arc::new(()),
            file_pages: Arc::new(FilePageRefs::new()),
        })
    }

    pub fn new_alloc(start: VirtAddr, size: PageSize) -> Self {
        Self::Cow(CowBackend {
            start,
            size,
            file: None,
            mapped_pages: Arc::new(AtomicUsize::new(0)),
            identity: Arc::new(()),
            file_pages: Arc::new(FilePageRefs::new()),
        })
    }
}
