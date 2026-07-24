use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    mem,
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use axerrno::{AxError, AxResult};
use axfs::{CachedFile, FileFlags, FolioPageRef};
use axhal::paging::{MappingFlags, PageSize, PageTable, PageTableMut, PagingError};
use axsync::Mutex;
use memory_addr::{MemoryAddr, PAGE_SIZE_4K, PhysAddr, VirtAddr, VirtAddrRange};

use super::{
    AddrSpace, AddrSpaceLock, Backend, BackendOps, FaultCommit, FaultPreparation, PopulateCallback,
    pages_in,
};

const NO_FAULT_PAGE: u32 = u32::MAX;
const MMAP_FAULT_READAHEAD_PAGES: u32 = 4;
const FILE_PAGE_SHARDS: usize = 64;

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
        &self.shards[vaddr.as_usize() / PAGE_SIZE_4K % FILE_PAGE_SHARDS]
    }

    fn insert(&self, vaddr: VirtAddr, page: FolioPageRef) {
        self.shard(vaddr).lock().insert(vaddr, page);
    }

    fn get(&self, vaddr: VirtAddr) -> Option<FolioPageRef> {
        self.shard(vaddr).lock().get(&vaddr).cloned()
    }

    fn remove(&self, vaddr: VirtAddr) -> Option<FolioPageRef> {
        self.shard(vaddr).lock().remove(&vaddr)
    }
}

#[doc(hidden)]
pub struct FileBackendInner {
    start: VirtAddr,
    cache: CachedFile,
    flags: FileFlags,
    offset_page: u32,
    handle: AtomicUsize,
    last_fault_page: AtomicU32,
    next_readahead_page: AtomicU32,
    futex_handle: Arc<()>,
    mapped_pages: FilePageRefs,
    pending_evictions: Mutex<Vec<(u32, FolioPageRef)>>,
    eviction_worker_running: AtomicBool,
}
impl Drop for FileBackendInner {
    fn drop(&mut self) {
        let handle = self.handle.load(Ordering::Acquire);
        if handle != 0 {
            unsafe {
                self.cache.remove_evict_listener(handle);
            }
        }
    }
}
impl FileBackendInner {
    pub fn register_listener(self: &Arc<Self>, aspace: &Arc<AddrSpaceLock>) {
        if self.handle.load(Ordering::Acquire) != 0 {
            panic!("Listener already registered");
        }
        let aspace = Arc::downgrade(aspace);
        let handle = self.cache.add_evict_listener({
            let this = Arc::downgrade(self);
            move |pn, page| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let Some(aspace) = aspace.upgrade() else {
                    // The address space has been dropped, nothing to do.
                    return;
                };
                if let Some(mut guard) = aspace.try_lock() {
                    this.on_evict(pn, page, &mut guard);
                } else {
                    this.defer_evict(pn, page.clone(), aspace);
                }
            }
        });
        self.handle.store(handle, Ordering::Release);
    }

    fn defer_evict(self: &Arc<Self>, pn: u32, page: FolioPageRef, aspace: Arc<AddrSpaceLock>) {
        self.pending_evictions.lock().push((pn, page));
        if self.eviction_worker_running.swap(true, Ordering::AcqRel) {
            return;
        }

        let this = self.clone();
        axtask::spawn(move || {
            loop {
                let pending = {
                    let mut pending = this.pending_evictions.lock();
                    mem::take(&mut *pending)
                };
                if pending.is_empty() {
                    this.eviction_worker_running.store(false, Ordering::Release);
                    if this.pending_evictions.lock().is_empty() {
                        return;
                    }
                    if this
                        .eviction_worker_running
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }

                let mut guard = aspace.lock();
                for (pn, page) in pending {
                    this.on_evict(pn, &page, &mut guard);
                }
            }
        });
    }

    fn on_evict(self: &Arc<Self>, pn: u32, page: &FolioPageRef, aspace: &mut AddrSpace) {
        let Some(pn) = pn.checked_sub(self.offset_page) else {
            return;
        };
        let vaddr = self.start + pn as usize * PageSize::Size4K as usize;
        if !aspace.find_area(vaddr).is_some_and(
            |it| matches!(it.backend(), Backend::File(file) if Arc::ptr_eq(&file.0, self)),
        ) {
            // Ignore if the page is not controlled by this file mapping.
            return;
        }
        if !self
            .mapped_pages
            .get(vaddr)
            .is_some_and(|mapped| mapped.paddr() == page.paddr())
        {
            return;
        }

        let pt = aspace.page_table_mut();
        match pt.query(vaddr) {
            Ok((paddr, _, PageSize::Size4K)) if paddr == page.paddr() => {}
            Ok(_) => return,
            Err(PagingError::NotMapped) => {
                self.mapped_pages.remove(vaddr);
                return;
            }
            Err(err) => {
                warn!("Failed to query file mapping {vaddr:?}: {err:?}");
                return;
            }
        }
        match pt.modify().unmap(vaddr) {
            Ok(_) => {}
            Err(PagingError::NotMapped) => {}
            Err(err) => {
                warn!("Failed to unmap page {:?}: {:?}", vaddr, err);
                return;
            }
        }
        self.mapped_pages.remove(vaddr);
    }
}

/// File-backed mapping backend.
#[derive(Clone)]
pub struct FileBackend(Arc<FileBackendInner>);

impl FileBackend {
    pub(super) fn merge_compatible(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub(crate) struct FileFaultPlan {
    identity: Arc<FileBackendInner>,
    vaddr: VirtAddr,
    pn: u32,
    page: Option<FolioPageRef>,
    expected_paddr: Option<PhysAddr>,
}

impl FileFaultPlan {
    pub(crate) fn execute(&mut self) -> AxResult {
        if self.expected_paddr.is_some() {
            return Ok(());
        }
        let offset = u64::from(self.pn) * PAGE_SIZE_4K as u64;
        if offset >= self.identity.cache.len()? {
            return Err(AxError::Io);
        }

        // The demanded page owns a naturally aligned folio load. Starting
        // advisory readahead first would let speculative requests occupy the
        // VirtIO queue while the faulting task waits behind them.
        self.page = Some(self.identity.cache.fault_page_ref(self.pn)?);
        Ok(())
    }
}

impl FileBackend {
    fn check_flags(&self, flags: MappingFlags) -> AxResult {
        let mut required_flags = FileFlags::empty();
        if flags.contains(MappingFlags::READ) {
            required_flags |= FileFlags::READ;
        }
        if flags.contains(MappingFlags::WRITE) {
            required_flags |= FileFlags::WRITE;
        }

        if !self.0.flags.contains(required_flags) {
            return Err(AxError::PermissionDenied);
        }
        Ok(())
    }

    pub fn futex_handle(&self) -> Weak<()> {
        Arc::downgrade(&self.0.futex_handle)
    }

    fn maybe_readahead_after_fault(&self, pn: u32) {
        let last = self.0.last_fault_page.swap(pn, Ordering::Relaxed);
        if last.checked_add(1) != Some(pn) {
            return;
        }

        let start = pn.saturating_add(1);
        if start < self.0.next_readahead_page.load(Ordering::Relaxed) {
            return;
        }

        let end = start.saturating_add(MMAP_FAULT_READAHEAD_PAGES);
        self.0.next_readahead_page.store(end, Ordering::Relaxed);
        let offset = u64::from(start) * PAGE_SIZE_4K as u64;
        let count = (end - start) as usize * PAGE_SIZE_4K;
        let _ = self.0.cache.readahead(offset, count);
    }

    pub(crate) fn prepare_fault(
        &self,
        vaddr: VirtAddr,
        _area_range: VirtAddrRange,
        _flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> FaultPreparation {
        let vaddr = vaddr.align_down(PAGE_SIZE_4K);
        match pt.query(vaddr) {
            Ok((_paddr, page_flags, PageSize::Size4K)) if page_flags.contains(access_flags) => {
                FaultPreparation::Handled
            }
            Ok((paddr, page_flags, PageSize::Size4K))
                if access_flags.contains(MappingFlags::WRITE)
                    && !page_flags.contains(MappingFlags::WRITE) =>
            {
                let Some(page) = self.0.mapped_pages.get(vaddr) else {
                    return FaultPreparation::Fallback;
                };
                if page.paddr() != paddr {
                    return FaultPreparation::Fallback;
                }
                let pn = ((vaddr - self.0.start) / PAGE_SIZE_4K) as u32 + self.0.offset_page;
                FaultPreparation::File(FileFaultPlan {
                    identity: self.0.clone(),
                    vaddr,
                    pn,
                    page: Some(page),
                    expected_paddr: Some(paddr),
                })
            }
            Ok(_) => FaultPreparation::Fallback,
            Err(PagingError::NotMapped) => {
                let pn = ((vaddr - self.0.start) / PAGE_SIZE_4K) as u32 + self.0.offset_page;
                FaultPreparation::File(FileFaultPlan {
                    identity: self.0.clone(),
                    vaddr,
                    pn,
                    page: None,
                    expected_paddr: None,
                })
            }
            Err(_) => FaultPreparation::Fallback,
        }
    }

    pub(crate) fn commit_fault(
        &self,
        plan: &mut FileFaultPlan,
        _area_range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> AxResult<FaultCommit> {
        if !Arc::ptr_eq(&self.0, &plan.identity) {
            return Ok(FaultCommit::Stale);
        }
        if let Some(expected_paddr) = plan.expected_paddr {
            match pt.query(plan.vaddr) {
                Ok((_paddr, page_flags, PageSize::Size4K)) if page_flags.contains(access_flags) => {
                    return Ok(FaultCommit::Handled);
                }
                Ok((paddr, page_flags, PageSize::Size4K))
                    if paddr == expected_paddr
                        && access_flags.contains(MappingFlags::WRITE)
                        && !page_flags.contains(MappingFlags::WRITE) => {}
                _ => return Ok(FaultCommit::Stale),
            }

            let page = plan.page.as_ref().ok_or(AxError::BadAddress)?;
            let committed = self.0.cache.with_valid_page_ref(plan.pn, page, true, || {
                pt.remap_4k_concurrent_if(plan.vaddr, expected_paddr, expected_paddr, flags)
            });
            return match committed {
                Some(Ok(true)) => Ok(FaultCommit::Handled),
                Some(Ok(false)) | None => Ok(FaultCommit::Stale),
                Some(Err(err)) => Err(err.into()),
            };
        }
        match pt.query(plan.vaddr) {
            Ok((_paddr, page_flags, PageSize::Size4K)) if page_flags.contains(access_flags) => {
                Ok(FaultCommit::Handled)
            }
            Ok(_) => Ok(FaultCommit::Stale),
            Err(PagingError::NotMapped) => {
                let page = plan.page.as_ref().ok_or(AxError::BadAddress)?;
                let mapping_page = page.clone();
                let write_fault = access_flags.contains(MappingFlags::WRITE);
                let map_flags = if self.0.cache.in_memory() || write_fault {
                    flags
                } else {
                    flags - MappingFlags::WRITE
                };
                let committed =
                    self.0
                        .cache
                        .with_valid_page_ref(plan.pn, page, write_fault, || {
                            match pt.map_4k_concurrent(plan.vaddr, page.paddr(), map_flags) {
                                Ok(()) => {
                                    self.0.mapped_pages.insert(plan.vaddr, mapping_page);
                                    Ok(true)
                                }
                                Err(PagingError::AlreadyMapped) => Ok(false),
                                Err(err) => Err(AxError::from(err)),
                            }
                        });
                match committed {
                    Some(Ok(true)) => {
                        plan.page = None;
                        Ok(FaultCommit::Handled)
                    }
                    Some(Ok(false)) | None => Ok(FaultCommit::Stale),
                    Some(Err(err)) => Err(err),
                }
            }
            Err(_) => Err(AxError::BadAddress),
        }
    }
}

impl BackendOps for FileBackend {
    fn page_size(&self) -> PageSize {
        PageSize::Size4K
    }

    fn map(&self, _range: VirtAddrRange, flags: MappingFlags, _pt: &mut PageTableMut) -> AxResult {
        self.check_flags(flags)
    }

    fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        for addr in pages_in(range, PageSize::Size4K)? {
            match pt.unmap(addr) {
                Ok(_) | Err(PagingError::NotMapped) => {
                    self.0.mapped_pages.remove(addr);
                }
                Err(err) => {
                    warn!("Failed to unmap page {:?}: {:?}", addr, err);
                    return Err(err.into());
                }
            }
        }
        Ok(())
    }

    fn discard(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        self.unmap(range, pt)
    }

    fn on_protect(
        &self,
        range: VirtAddrRange,
        new_flags: MappingFlags,
        _pt: &mut PageTableMut,
    ) -> AxResult {
        self.check_flags(new_flags)?;
        if new_flags.contains(MappingFlags::WRITE) && !self.0.cache.in_memory() {
            let start_page =
                ((range.start - self.0.start) / PAGE_SIZE_4K) as u32 + self.0.offset_page;
            for (index, addr) in pages_in(range, PageSize::Size4K)?.enumerate() {
                if self.0.mapped_pages.get(addr).is_none() {
                    continue;
                }
                let pn = start_page + index as u32;
                self.0.cache.with_page(pn, |page| {
                    if let Some(page) = page {
                        page.mark_dirty();
                    }
                });
            }
        }
        Ok(())
    }

    fn populate(
        &self,
        range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult<(usize, Option<PopulateCallback>)> {
        let mut pages = 0;
        let start_page = ((range.start - self.0.start) / PAGE_SIZE_4K) as u32 + self.0.offset_page;
        let fault_readahead =
            range.end - range.start == PAGE_SIZE_4K && access_flags.contains(MappingFlags::READ);
        let file_len = self.0.cache.len().ok();
        for (i, addr) in pages_in(range, PageSize::Size4K)?.enumerate() {
            let pn = start_page + i as u32;
            if file_len.is_some_and(|len| u64::from(pn) * PAGE_SIZE_4K as u64 >= len) {
                return Err(AxError::Io);
            }
            match pt.query(addr) {
                Ok((paddr, page_flags, _)) => {
                    if access_flags.contains(MappingFlags::WRITE)
                        && !page_flags.contains(MappingFlags::WRITE)
                    {
                        let in_memory = self.0.cache.in_memory();
                        self.0.cache.with_page(pn, |page| {
                            if !in_memory {
                                page.expect("page should be present").mark_dirty();
                            }
                            pt.remap(addr, paddr, flags)?;
                            pages += 1;
                            AxResult::Ok(())
                        })?;
                    } else if page_flags.contains(access_flags) {
                        pages += 1;
                    }
                }
                // If the page is not mapped, try map it.
                Err(PagingError::NotMapped) => {
                    let map_flags = if self.0.cache.in_memory() {
                        // For in memory files, we don't need to (and also
                        // musn't) mark them dirty, so we can use the original
                        // flags.
                        flags
                    } else {
                        flags - MappingFlags::WRITE
                    };
                    self.0.cache.with_page_or_insert(pn, |page, _| {
                        pt.map(addr, page.paddr(), PageSize::Size4K, map_flags)?;
                        self.0.mapped_pages.insert(addr, page.page_ref());
                        pages += 1;
                        Ok(())
                    })?;
                }
                Err(_) => return Err(AxError::BadAddress),
            }
        }
        if pages > 0 && fault_readahead {
            self.maybe_readahead_after_fault(start_page);
        }
        Ok((pages, None))
    }

    fn clone_map(
        &self,
        _range: VirtAddrRange,
        _flags: MappingFlags,
        _old_pt: &mut PageTableMut,
        _new_pt: &mut PageTableMut,
        new_aspace: &Arc<AddrSpaceLock>,
    ) -> AxResult<Backend> {
        let inner = Arc::new(FileBackendInner {
            start: self.0.start,
            cache: self.0.cache.clone(),
            flags: self.0.flags,
            offset_page: self.0.offset_page,
            handle: AtomicUsize::new(0),
            last_fault_page: AtomicU32::new(NO_FAULT_PAGE),
            next_readahead_page: AtomicU32::new(0),
            futex_handle: self.0.futex_handle.clone(),
            mapped_pages: FilePageRefs::new(),
            pending_evictions: Mutex::new(Vec::new()),
            eviction_worker_running: AtomicBool::new(false),
        });
        inner.register_listener(new_aspace);
        Ok(Backend::File(FileBackend(inner)))
    }
}

impl Backend {
    pub fn new_file(
        start: VirtAddr,
        cache: CachedFile,
        flags: FileFlags,
        offset: usize,
        aspace: &Arc<AddrSpaceLock>,
    ) -> Self {
        let offset_page = (offset / PAGE_SIZE_4K) as u32;
        let inner = Arc::new(FileBackendInner {
            start,
            cache,
            flags,
            offset_page,
            handle: AtomicUsize::new(0),
            last_fault_page: AtomicU32::new(NO_FAULT_PAGE),
            next_readahead_page: AtomicU32::new(0),
            futex_handle: Arc::new(()),
            mapped_pages: FilePageRefs::new(),
            pending_evictions: Mutex::new(Vec::new()),
            eviction_worker_running: AtomicBool::new(false),
        });
        inner.register_listener(aspace);
        Self::File(FileBackend(inner))
    }
}
