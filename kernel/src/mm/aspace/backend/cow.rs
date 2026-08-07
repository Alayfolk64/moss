use alloc::{collections::BTreeMap, sync::Arc};
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
    UnmapGather, alloc_frame, dealloc_frame, pages_in,
};

struct FrameRefCnt(usize);

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
    frame_table(frame).lock().init_frame(frame);
    Ok(frame)
}

pub(super) fn drop_referenced_frame(frame: PhysAddr, size: PageSize) {
    let frame_ref = frame_table(frame)
        .lock()
        .get_frame_ref(frame)
        .expect("referenced frame must be present");
    frame_ref.lock().drop_frame(frame, size);
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
            } else if frame_table(paddr).lock().get_frame_ref(paddr).is_none() {
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
                        let Some(frame_ref) = frame_table(paddr).lock().get_frame_ref(paddr) else {
                            return FaultPreparation::Fallback;
                        };
                        let mut frame_ref = frame_ref.lock();
                        if frame_ref.0 == 1 {
                            return FaultPreparation::Fallback;
                        }
                        let Some(count) = frame_ref.0.checked_add(1) else {
                            return FaultPreparation::Fallback;
                        };
                        frame_ref.0 = count;
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
