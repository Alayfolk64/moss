use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use axerrno::{AxError, AxResult};
use axfs::{CachedFile, FileFlags, PageEviction};
use axhal::paging::{MappingFlags, PageSize, PageTableMut, PagingError};
use memory_addr::{MemoryAddr, PAGE_SIZE_4K, VirtAddr, VirtAddrRange};

use super::{
    AddrSpace, AddrSpaceLock, Backend, BackendOps, FaultPreparation, PopulateCallback,
    finish_unmap_before_reclaim, pages_in,
};

const NO_FAULT_PAGE: u32 = u32::MAX;
const MMAP_FAULT_READAHEAD_PAGES: u32 = 4;

#[doc(hidden)]
pub struct FileBackendInner {
    start: VirtAddr,
    cache: CachedFile,
    flags: FileFlags,
    offset_page: u32,
    owner: usize,
    handle: AtomicUsize,
    last_fault_page: AtomicU32,
    next_readahead_page: AtomicU32,
    futex_handle: Arc<()>,
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
        let handle = self.cache.add_evict_listener_checked(self.owner, {
            let this = Arc::downgrade(self);
            move |pn, paddr, wait| {
                let Some(this) = this.upgrade() else {
                    return true;
                };
                let Some(aspace) = aspace.upgrade() else {
                    // The address space has been dropped, nothing to do.
                    return true;
                };
                if wait {
                    // Sync/writeback callers hold no address-space lock.  Use
                    // normal blocking semantics so transient mmap activity does
                    // not escape to fsync as `WouldBlock`.
                    return this.on_evict(pn, paddr, &mut aspace.lock());
                }
                // Ordinary cache insertion can run while MAP_POPULATE or a
                // fallback fault already holds another address-space lock.  Do
                // not wait in that mode: two mappings of the same inode could
                // otherwise acquire A -> B and B -> A.
                let Some(mut aspace) = aspace.try_lock() else {
                    return false;
                };
                this.on_evict(pn, paddr, &mut aspace)
            }
        });
        self.handle.store(handle, Ordering::Release);
    }

    fn evicted_vaddr(&self, pn: u32) -> Option<VirtAddr> {
        let Some(pn) = pn.checked_sub(self.offset_page) else {
            return None;
        };
        Some(self.start + pn as usize * PageSize::Size4K as usize)
    }

    fn unmap_matching_page(
        &self,
        pn: u32,
        paddr: axhal::mem::PhysAddr,
        pt: &mut PageTableMut,
    ) -> bool {
        let Some(vaddr) = self.evicted_vaddr(pn) else {
            return true;
        };
        match pt.query(vaddr) {
            Ok((mapped, ..)) if mapped == paddr => match pt.unmap(vaddr) {
                Ok(_) => {
                    finish_unmap_before_reclaim(pt);
                    true
                }
                Err(PagingError::NotMapped) => true,
                Err(err) => {
                    warn!("Failed to unmap page {:?}: {:?}", vaddr, err);
                    false
                }
            },
            Ok(_) | Err(PagingError::NotMapped) => true,
            Err(err) => {
                warn!("Failed to query evicted page {:?}: {:?}", vaddr, err);
                false
            }
        }
    }

    fn on_evict(
        self: &Arc<Self>,
        pn: u32,
        paddr: axhal::mem::PhysAddr,
        aspace: &mut AddrSpace,
    ) -> bool {
        let Some(vaddr) = self.evicted_vaddr(pn) else {
            return true;
        };
        if !aspace.find_area(vaddr).is_some_and(
            |it| matches!(it.backend(), Backend::File(file) if Arc::ptr_eq(&file.0, self)),
        ) {
            // Ignore if the page is not controlled by this file mapping.
            return true;
        }

        let mut modify = aspace.page_table_mut().modify();
        self.unmap_matching_page(pn, paddr, &mut modify)
    }

    fn finish_evicted_pages(self: &Arc<Self>, pages: Vec<PageEviction>, aspace: &mut AddrSpace) {
        for eviction in pages {
            let others_done = self.cache.notify_evicted_page_except(eviction, self.owner);
            let current_done = aspace.unmap_cached_file_page(
                &self.cache,
                eviction.page_number(),
                eviction.paddr(),
            );
            if let Err(err) = self
                .cache
                .complete_evicted_page(eviction, others_done && current_done)
            {
                warn!("Failed to complete mmap page eviction: {err:?}");
            }
        }
    }

    fn finish_evicted_pages_in_table(&self, pages: Vec<PageEviction>, pt: &mut PageTableMut) {
        for eviction in pages {
            let current_done =
                self.unmap_matching_page(eviction.page_number(), eviction.paddr(), pt);
            // This error path only has the current backend and PageTableMut, so
            // it cannot enumerate sibling VMAs belonging to the same owner.
            // Notify every checked listener.  Listeners for the currently
            // locked address space return false, keeping the token pinned; a
            // later retry obtains that address space and removes all aliases.
            let all_listeners_done = self.cache.notify_evicted_page(eviction);
            if let Err(err) = self
                .cache
                .complete_evicted_page(eviction, all_listeners_done && current_done)
            {
                warn!("Failed to complete mmap page eviction: {err:?}");
            }
        }
    }
}

/// File-backed mapping backend.
#[derive(Clone)]
pub struct FileBackend(Arc<FileBackendInner>);
impl FileBackend {
    pub(crate) fn prepare_fault(&self, vaddr: VirtAddr) -> FaultPreparation {
        let page = ((vaddr.align_down_4k() - self.0.start) / PAGE_SIZE_4K) as u32;
        let Some(pn) = self.0.offset_page.checked_add(page) else {
            return FaultPreparation::Fallback;
        };
        if self.0.cache.page_eviction_in_progress(pn) {
            FaultPreparation::Retry
        } else {
            FaultPreparation::Fallback
        }
    }

    fn evicted_vaddr_for_cache(&self, cache: &CachedFile, pn: u32) -> Option<VirtAddr> {
        self.0
            .cache
            .ptr_eq(cache)
            .then(|| self.0.evicted_vaddr(pn))
            .flatten()
    }

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
}

impl BackendOps for FileBackend {
    fn page_size(&self) -> PageSize {
        PageSize::Size4K
    }

    fn map(&self, _range: VirtAddrRange, flags: MappingFlags, _pt: &mut PageTableMut) -> AxResult {
        self.check_flags(flags)
    }

    fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        let result = (|| {
            for addr in pages_in(range, PageSize::Size4K)? {
                match pt.unmap(addr) {
                    Ok(_) | Err(PagingError::NotMapped) => {}
                    Err(err) => {
                        warn!("Failed to unmap page {:?}: {:?}", addr, err);
                        return Err(err.into());
                    }
                }
            }
            Ok(())
        })();
        finish_unmap_before_reclaim(pt);
        result
    }

    fn on_protect(
        &self,
        _range: VirtAddrRange,
        new_flags: MappingFlags,
        _pt: &mut PageTableMut,
    ) -> AxResult<MappingFlags> {
        self.check_flags(new_flags)?;
        Ok(new_flags)
    }

    fn populate(
        &self,
        range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult<(usize, Option<PopulateCallback>)> {
        let mut pages = 0;
        let mut to_be_evicted: Vec<PageEviction> = Vec::new();
        let start_page = ((range.start - self.0.start) / PAGE_SIZE_4K) as u32 + self.0.offset_page;
        let fault_readahead =
            range.end - range.start == PAGE_SIZE_4K && access_flags.contains(MappingFlags::READ);
        let file_len = self.0.cache.len().ok();
        let populate_result: AxResult = (|| {
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
                                let Some(page) = page else {
                                    return Err(AxError::WouldBlock);
                                };
                                if !in_memory {
                                    page.mark_dirty();
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
                        let (map_result, evicted) =
                            self.0.cache.with_page_or_insert_deferred(pn, |page| {
                                pt.map(addr, page.paddr(), PageSize::Size4K, map_flags)
                            })?;
                        if let Some(evicted) = evicted {
                            to_be_evicted.push(evicted);
                        }
                        map_result?;
                        pages += 1;
                    }
                    Err(_) => return Err(AxError::BadAddress),
                }
            }
            Ok(())
        })();
        if let Err(err) = populate_result {
            self.0.finish_evicted_pages_in_table(to_be_evicted, pt);
            return Err(err);
        }
        // Readahead can itself evict a page.  It must not run while this
        // address-space lock is held because a checked eviction listener may
        // synchronously wait for this same address space.  A later stage can
        // move deferred readahead into the post-populate callback.
        let _ = fault_readahead;
        Ok((
            pages,
            if to_be_evicted.is_empty() {
                None
            } else {
                let inner = self.0.clone();
                Some(Box::new(move |aspace: &mut AddrSpace| {
                    inner.finish_evicted_pages(to_be_evicted, aspace);
                }))
            },
        ))
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
            owner: new_aspace.eviction_owner(),
            handle: AtomicUsize::new(0),
            last_fault_page: AtomicU32::new(NO_FAULT_PAGE),
            next_readahead_page: AtomicU32::new(0),
            futex_handle: self.0.futex_handle.clone(),
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
            owner: aspace.eviction_owner(),
            handle: AtomicUsize::new(0),
            last_fault_page: AtomicU32::new(NO_FAULT_PAGE),
            next_readahead_page: AtomicU32::new(0),
            futex_handle: Arc::new(()),
        });
        inner.register_listener(aspace);
        Self::File(FileBackend(inner))
    }
}

impl AddrSpace {
    /// Removes every mapping in this address space that points at the exact
    /// detached file-cache frame.  One address space may map the same inode
    /// through several VMAs/listeners; batching them here avoids re-entering
    /// the address-space lock through a sibling listener.
    fn unmap_cached_file_page(
        &mut self,
        cache: &CachedFile,
        pn: u32,
        paddr: axhal::mem::PhysAddr,
    ) -> bool {
        let vaddrs = self
            .areas
            .iter()
            .filter_map(|area| match area.backend() {
                Backend::File(file) => file
                    .evicted_vaddr_for_cache(cache, pn)
                    .filter(|vaddr| area.va_range().contains(*vaddr)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut complete = true;
        let mut changed = false;
        let mut modify = self.page_table_mut().modify();
        for vaddr in vaddrs {
            match modify.query(vaddr) {
                Ok((mapped, ..)) if mapped == paddr => match modify.unmap(vaddr) {
                    Ok(_) => changed = true,
                    Err(PagingError::NotMapped) => {}
                    Err(err) => {
                        warn!("Failed to unmap page {:?}: {:?}", vaddr, err);
                        complete = false;
                    }
                },
                Ok(_) | Err(PagingError::NotMapped) => {}
                Err(err) => {
                    warn!("Failed to query evicted page {:?}: {:?}", vaddr, err);
                    complete = false;
                }
            }
        }
        if changed {
            finish_unmap_before_reclaim(&mut modify);
        }
        complete
    }
}
