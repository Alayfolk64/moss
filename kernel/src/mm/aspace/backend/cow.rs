use alloc::{collections::BTreeMap, sync::Arc};
use core::{
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};

use axerrno::{AxError, AxResult};
use axfs::FileBackend;
use axhal::{
    mem::phys_to_virt,
    paging::{MappingFlags, PageSize, PageTableMut, PagingError},
};
use axsync::Mutex;
use kspin::SpinNoIrq;
use memory_addr::{PhysAddr, VirtAddr, VirtAddrRange};

use super::{
    AddrSpace, Backend, BackendOps, PopulateCallback, alloc_frame, dealloc_frame, pages_in,
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
            FRAME_TABLE.lock().remove_frame(paddr);
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

static FRAME_TABLE: SpinNoIrq<FrameTableRefCount> = SpinNoIrq::new(FrameTableRefCount::new());

/// Copy-on-write mapping backend.
///
/// This corresponds to the `MAP_PRIVATE` flag.
pub struct CowBackend {
    start: VirtAddr,
    size: PageSize,
    file: Option<(FileBackend, u64, Option<u64>)>,
    mapped_pages: AtomicUsize,
}

impl Clone for CowBackend {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            mapped_pages: AtomicUsize::new(self.mapped_pages.load(Ordering::Relaxed)),
        }
    }
}

impl CowBackend {
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
        let frame = alloc_frame(zeroed, self.size)?;
        FRAME_TABLE.lock().init_frame(frame);
        Ok(frame)
    }

    fn drop_new_frame(&self, frame: PhysAddr) {
        let frame_ref = FRAME_TABLE
            .lock()
            .get_frame_ref(frame)
            .expect("new frame must be referenced");
        frame_ref.lock().drop_frame(frame, self.size);
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
        let frame = FRAME_TABLE
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
                    if page_size != self.size {
                        warn!("rollback COW clone got unexpected page size: {page_size:?}");
                        continue;
                    }

                    let Some(frame_ref) = FRAME_TABLE.lock().get_frame_ref(frame) else {
                        warn!("rollback COW clone found unreferenced frame: {frame:?}");
                        continue;
                    };
                    frame_ref.lock().drop_frame(frame, self.size);
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
        let mut frame_table = FRAME_TABLE.lock();
        let frame = frame_table
            .get_frame_ref(paddr)
            .ok_or(AxError::BadAddress)?;
        drop(frame_table);
        let mut frame = frame.lock();
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
                frame.drop_frame(paddr, self.size);
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
                let frame_ref = FRAME_TABLE
                    .lock()
                    .get_frame_ref(frame)
                    .ok_or(AxError::BadAddress)?;
                let mut frame_ref = frame_ref.lock();
                frame_ref.drop_frame(frame, self.size);
                self.dec_mapped_pages();
            } else {
                // Deallocation is needn't if the page is not allocated.
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
        _new_aspace: &Arc<Mutex<AddrSpace>>,
    ) -> AxResult<Backend> {
        // Forking untouched lazy COW regions is common (e.g. fork14). In that
        // case, there is nothing in page tables to duplicate.
        if self.mapped_pages.load(Ordering::Relaxed) == 0 {
            return Ok(Backend::Cow(self.clone()));
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
                    if let Err(err) =
                        self.share_existing_at(vaddr, paddr, cow_flags, old_pt, new_pt)
                    {
                        self.rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                        return Err(err);
                    }
                }
                // If the page is not mapped, skip it.
                Err(PagingError::NotMapped) => {}
                Err(_) => {
                    self.rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                    return Err(AxError::BadAddress);
                }
            };
        }

        Ok(Backend::Cow(self.clone()))
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
        })
    }

    pub fn new_alloc(start: VirtAddr, size: PageSize) -> Self {
        Self::Cow(CowBackend {
            start,
            size,
            file: None,
            mapped_pages: AtomicUsize::new(0),
        })
    }
}
