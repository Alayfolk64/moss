use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};

use axerrno::{AxError, AxResult};
use axfs::{FileBackend, FileLengthChangeListener, FileLengthChangePhase};
use axhal::{
    mem::phys_to_virt,
    paging::{MappingFlags, PageSize, PageTable, PageTableMut, PagingError},
};
use axsync::Mutex;
use kspin::SpinNoIrq;
use memory_addr::{MemoryAddr, PhysAddr, VirtAddr, VirtAddrRange};

use super::{
    AddrSpace, AddrSpaceLock, Backend, BackendOps, FaultCommit, FaultPreparation, PopulateCallback,
    alloc_frame, dealloc_frame, finish_unmap_before_reclaim, pages_in,
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
    mapped_pages: Arc<AtomicUsize>,
    identity: Arc<()>,
    length_listener: Arc<Mutex<Option<FileLengthChangeListener>>>,
}

impl Clone for CowBackend {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            mapped_pages: self.mapped_pages.clone(),
            identity: self.identity.clone(),
            length_listener: self.length_listener.clone(),
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

fn file_page_offset(mapping_start: VirtAddr, vaddr: VirtAddr, file_start: u64) -> u64 {
    file_start + vaddr.as_usize().saturating_sub(mapping_start.as_usize()) as u64
}

/// Returns the destination offset, file offset and exact number of bytes which
/// belong to this file-backed page.  `Some(file_end)` denotes an ELF segment:
/// pages after that segment's file data are valid zero-filled BSS.  Ordinary
/// mmap passes `None`, so a page at or beyond the current EOF is a fault.
fn file_read_window(
    file: &FileBackend,
    mapping_start: VirtAddr,
    vaddr: VirtAddr,
    mapping_file_start: u64,
    file_end: Option<u64>,
    page_len: usize,
) -> AxResult<(usize, u64, usize)> {
    let dst_start = mapping_start.as_usize().saturating_sub(vaddr.as_usize());
    assert!(dst_start < page_len);
    let fault_file_start = file_page_offset(mapping_start, vaddr, mapping_file_start);

    if file_end.is_some_and(|end| fault_file_start >= end) {
        return Ok((dst_start, fault_file_start, 0));
    }
    let readable_end = file_end.unwrap_or(u64::MAX).min(file.len()?);
    if fault_file_start >= readable_end {
        return Err(AxError::BadAddress);
    }
    let read_len = readable_end
        .saturating_sub(fault_file_start)
        .min((page_len - dst_start) as u64) as usize;
    Ok((dst_start, fault_file_start, read_len))
}

fn read_file_window_exact(
    file: &FileBackend,
    buf: &mut [u8],
    dst_start: usize,
    file_start: u64,
    read_len: usize,
    no_evict: bool,
) -> AxResult {
    let mut filled = 0;
    while filled < read_len {
        let dst = &mut &mut buf[dst_start + filled..dst_start + read_len];
        let read = if no_evict {
            file.read_at_no_evict(dst, file_start + filled as u64)?
        } else {
            file.read_at(dst, file_start + filled as u64)?
        };
        if read == 0 || read > read_len - filled {
            return Err(AxError::Io);
        }
        filled += read;
    }
    Ok(())
}

enum CowFaultSource {
    /// An extra reference pins an anonymous COW source while the address-space
    /// lock is released.
    Anonymous(PhysAddr),
}

impl CowFaultSource {
    fn paddr(&self) -> PhysAddr {
        match self {
            Self::Anonymous(paddr) => *paddr,
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
    file_length_generation: Option<usize>,
    frame: Option<PhysAddr>,
    source: Option<CowFaultSource>,
}

impl CowFaultPlan {
    fn file_length_is_current(&self) -> bool {
        let Some(generation) = self.file_length_generation else {
            return true;
        };
        let Some((file, mapping_file_start, _)) = &self.file else {
            return false;
        };
        let offset = file_page_offset(self.mapping_start, self.vaddr, *mapping_file_start);
        file.length_change_generation(offset) == Some(generation)
    }

    pub(crate) fn execute(&mut self) -> AxResult {
        if let Some(source) = self.source.as_ref().map(CowFaultSource::paddr) {
            if !self.file_length_is_current() {
                return Err(AxError::WouldBlock);
            }
            let frame = alloc_referenced_frame(false, self.size)?;
            self.frame = Some(frame);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(source).as_ptr(),
                    phys_to_virt(frame).as_mut_ptr(),
                    self.size as _,
                );
            }
            if !self.file_length_is_current() {
                return Err(AxError::WouldBlock);
            }
            return Ok(());
        }

        let frame = alloc_referenced_frame(true, self.size)?;
        self.frame = Some(frame);
        if let Some((file, mapping_file_start, file_end)) = &self.file {
            let buf = unsafe {
                slice::from_raw_parts_mut(phys_to_virt(frame).as_mut_ptr(), self.size as _)
            };
            let (start, fault_file_start, read_len) = file_read_window(
                file,
                self.mapping_start,
                self.vaddr,
                *mapping_file_start,
                *file_end,
                buf.len(),
            )?;
            if read_len != 0 {
                let generation = self.file_length_generation.ok_or(AxError::BadAddress)?;
                if file.length_change_generation(fault_file_start) != Some(generation) {
                    return Err(AxError::WouldBlock);
                }
            }
            read_file_window_exact(file, buf, start, fault_file_start, read_len, false)?;
            if read_len != 0
                && file.length_change_generation(fault_file_start) != self.file_length_generation
            {
                return Err(AxError::WouldBlock);
            }
        }
        Ok(())
    }
}

impl Drop for CowFaultPlan {
    fn drop(&mut self) {
        if let Some(frame) = self.frame.take() {
            drop_referenced_frame(frame, self.size);
        }
        if let Some(CowFaultSource::Anonymous(frame)) = self.source.take() {
            drop_referenced_frame(frame, self.size);
        }
    }
}

fn alloc_referenced_frame(zeroed: bool, size: PageSize) -> AxResult<PhysAddr> {
    let frame = alloc_frame(zeroed, size)?;
    FRAME_TABLE.lock().init_frame(frame);
    Ok(frame)
}

fn drop_referenced_frame(frame: PhysAddr, size: PageSize) {
    let frame_ref = FRAME_TABLE
        .lock()
        .get_frame_ref(frame)
        .expect("referenced frame must be present");
    frame_ref.lock().drop_frame(frame, size);
}

impl CowBackend {
    fn clone_for_fork(&self, new_aspace: &Arc<AddrSpaceLock>) -> Self {
        let backend = Self {
            start: self.start,
            size: self.size,
            file: self.file.clone(),
            mapped_pages: Arc::new(AtomicUsize::new(0)),
            identity: Arc::new(()),
            length_listener: Arc::new(Mutex::new(None)),
        };
        backend.ensure_length_listener(new_aspace);
        backend
    }

    pub(super) fn ensure_length_listener(&self, aspace: &Arc<AddrSpaceLock>) {
        let Some((file, ..)) = &self.file else {
            return;
        };
        let mut slot = self.length_listener.lock();
        if slot.is_some() {
            return;
        }

        let weak_aspace = Arc::downgrade(aspace);
        let weak_identity: Weak<()> = Arc::downgrade(&self.identity);
        *slot = Some(file.add_length_change_listener(move |range, phase| {
            let (Some(aspace), Some(identity)) = (weak_aspace.upgrade(), weak_identity.upgrade())
            else {
                return;
            };
            let mut guard = aspace.lock();
            let AddrSpace { areas, pt, .. } = &mut *guard;
            for area in areas.iter() {
                let Backend::Cow(backend) = area.backend() else {
                    continue;
                };
                if Arc::ptr_eq(&backend.identity, &identity) {
                    backend.invalidate_file_pages(area.va_range(), range.clone(), phase, pt);
                }
            }
        }));
    }

    fn file_page_is_current(&self, vaddr: VirtAddr, generation: Option<usize>) -> AxResult<bool> {
        let Some((file, mapping_file_start, file_end)) = &self.file else {
            return Ok(true);
        };
        let offset = file_page_offset(self.start, vaddr, *mapping_file_start);
        if file_end.is_some_and(|end| offset >= end) {
            return Ok(true);
        }
        if let Some(generation) = generation {
            if file.length_change_generation(offset) != Some(generation) {
                return Err(AxError::WouldBlock);
            }
        } else if file.length_change_in_progress(offset) {
            return Err(AxError::WouldBlock);
        }
        Ok(offset < file.len()?)
    }

    fn file_page_change_in_progress(&self, vaddr: VirtAddr) -> bool {
        let Some((file, mapping_file_start, file_end)) = &self.file else {
            return false;
        };
        let offset = file_page_offset(self.start, vaddr, *mapping_file_start);
        !file_end.is_some_and(|end| offset >= end) && file.length_change_in_progress(offset)
    }

    pub(super) fn file_length_change_in_progress(&self, range: VirtAddrRange) -> bool {
        pages_in(range, self.size)
            .is_ok_and(|mut pages| pages.any(|vaddr| self.file_page_change_in_progress(vaddr)))
    }

    fn invalidate_file_pages(
        &self,
        area_range: VirtAddrRange,
        file_range: core::ops::Range<u64>,
        phase: FileLengthChangePhase,
        pt: &mut PageTable,
    ) {
        let Some((_, mapping_file_start, file_end)) = &self.file else {
            return;
        };
        let Ok(pages) = pages_in(area_range, self.size) else {
            return;
        };
        let mut removed = Vec::new();
        let mut protected = false;
        let mut modify = pt.modify();
        for vaddr in pages {
            let offset = file_page_offset(self.start, vaddr, *mapping_file_start);
            let dst_start = self.start.as_usize().saturating_sub(vaddr.as_usize());
            let mapped_end = offset
                .saturating_add((self.size as usize - dst_start) as u64)
                .min(file_end.unwrap_or(u64::MAX));
            if offset >= file_range.end || mapped_end <= file_range.start {
                continue;
            }

            match modify.query(vaddr) {
                Ok((frame, flags, page_size)) => {
                    assert_eq!(page_size, self.size);
                    if file_range.start <= offset {
                        if phase == FileLengthChangePhase::Apply {
                            match modify.unmap(vaddr) {
                                Ok((frame, _, page_size)) => {
                                    assert_eq!(page_size, self.size);
                                    removed.push(frame);
                                }
                                Err(err) => panic!(
                                    "failed to revoke truncated private page {vaddr:?}: {err:?}"
                                ),
                            }
                        }
                    } else if phase == FileLengthChangePhase::Protect {
                        // The file layer invokes Protect for every listener
                        // before any Apply callback.  Removing WRITE and
                        // finishing this address space's remote TLB barrier
                        // contributes to that global fork-shared-frame fence.
                        modify
                            .protect(vaddr, flags - MappingFlags::WRITE)
                            .unwrap_or_else(|err| {
                                panic!("failed to protect partial private page {vaddr:?}: {err:?}")
                            });
                        protected = true;
                    } else {
                        // All listeners have already completed Protect, so no
                        // address space may retain a writable translation to
                        // this possibly fork-shared frame.  Preserve the
                        // private prefix and zero only bytes after the new EOF.
                        let zero_from = dst_start + (file_range.start - offset) as usize;
                        let data = unsafe {
                            slice::from_raw_parts_mut(
                                phys_to_virt(frame).as_mut_ptr(),
                                self.size as usize,
                            )
                        };
                        data[zero_from..].fill(0);
                    }
                }
                Err(PagingError::NotMapped) => {}
                Err(err) => panic!("failed to inspect truncated private page {vaddr:?}: {err:?}"),
            }
        }
        if protected || !removed.is_empty() {
            finish_unmap_before_reclaim(&mut modify);
        }
        for frame in removed {
            drop_referenced_frame(frame, self.size);
            self.dec_mapped_pages();
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
        aspace: &Arc<AddrSpaceLock>,
        vaddr: VirtAddr,
        _area_range: VirtAddrRange,
        _flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &PageTable,
    ) -> FaultPreparation {
        self.ensure_length_listener(aspace);
        if self.size != PageSize::Size4K {
            return FaultPreparation::Fallback;
        }
        let vaddr = vaddr.align_down(self.size as usize);
        if self.file_page_change_in_progress(vaddr) {
            return FaultPreparation::Retry;
        }
        let file_length_generation = match &self.file {
            Some((file, mapping_file_start, file_end)) => {
                let offset = file_page_offset(self.start, vaddr, *mapping_file_start);
                if file_end.is_some_and(|end| offset >= end) {
                    None
                } else {
                    let Some(generation) = file.length_change_generation(offset) else {
                        return FaultPreparation::Retry;
                    };
                    Some(generation)
                }
            }
            None => None,
        };
        match pt.query(vaddr) {
            Ok((paddr, page_flags, page_size)) => {
                if page_size == self.size && page_flags.contains(access_flags) {
                    FaultPreparation::Handled
                } else if page_size == self.size
                    && access_flags.contains(MappingFlags::WRITE)
                    && !page_flags.contains(MappingFlags::WRITE)
                {
                    // Concurrent replacement of an existing COW mapping needs
                    // a synchronous remote TLB invalidation before releasing
                    // the old PTE reference. Other architectures keep the
                    // exclusive path until they provide that primitive.
                    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
                    return FaultPreparation::Fallback;

                    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                    {
                        let Some(frame_ref) = FRAME_TABLE.lock().get_frame_ref(paddr) else {
                            return FaultPreparation::Fallback;
                        };
                        let mut frame_ref = frame_ref.lock();
                        // A zero count means the last PTE reference already
                        // removed this frame from the lifetime protocol. It
                        // must never be resurrected after physical reuse.
                        if frame_ref.0 <= 1 {
                            return FaultPreparation::Fallback;
                        }
                        let Some(count) = frame_ref.0.checked_add(1) else {
                            return FaultPreparation::Fallback;
                        };
                        frame_ref.0 = count;
                        return FaultPreparation::Cow(CowFaultPlan {
                            identity: self.identity.clone(),
                            vaddr,
                            mapping_start: self.start,
                            size: self.size,
                            file: self.file.clone(),
                            file_length_generation,
                            frame: None,
                            source: Some(CowFaultSource::Anonymous(paddr)),
                        });
                    }
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
                file_length_generation,
                frame: None,
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
        match self.file_page_is_current(plan.vaddr, plan.file_length_generation) {
            Ok(true) => {}
            Ok(false) => return Err(AxError::BadAddress),
            Err(AxError::WouldBlock) => return Ok(FaultCommit::Stale),
            Err(err) => return Err(err),
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
                    if !pt.remap_4k_concurrent_if(plan.vaddr, paddr, frame, flags)? {
                        return Ok(FaultCommit::Stale);
                    }
                    // The leaf CAS replaced the physical frame. Before the
                    // old PTE reference can be released, every CPU running
                    // this root must stop using a cached translation to it.
                    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                    crate::mm::flush_tlb_on_all_cpus(pt.root_paddr(), Some(plan.vaddr));
                    // Drop the reference formerly owned by this PTE. The
                    // plan's extra pin remains until CowFaultPlan is dropped.
                    drop_referenced_frame(paddr, self.size);
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
            let (start, file_start, read_len) = match file_read_window(
                file,
                self.start,
                vaddr,
                *file_start,
                *file_end,
                buf.len(),
            ) {
                Ok(window) => window,
                Err(err) => {
                    self.drop_new_frame(frame);
                    return Err(err);
                }
            };
            let generation = if read_len == 0 {
                None
            } else {
                let Some(generation) = file.length_change_generation(file_start) else {
                    self.drop_new_frame(frame);
                    return Err(AxError::WouldBlock);
                };
                Some(generation)
            };
            if let Err(err) = read_file_window_exact(file, buf, start, file_start, read_len, true) {
                self.drop_new_frame(frame);
                return Err(err);
            }
            if let Some(generation) = generation
                && file.length_change_generation(file_start) != Some(generation)
            {
                self.drop_new_frame(frame);
                return Err(AxError::WouldBlock);
            }
            match self.file_page_is_current(vaddr, generation) {
                Ok(true) => {}
                Ok(false) => {
                    self.drop_new_frame(frame);
                    return Err(AxError::BadAddress);
                }
                Err(err) => {
                    self.drop_new_frame(frame);
                    return Err(err);
                }
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
        let ref_count = {
            let frame = frame.lock();
            assert!(frame.0 > 0, "invalid frame reference count");
            frame.0
        };
        match ref_count {
            1 => {
                // Only one reference, just upgrade the permissions.
                pt.protect(vaddr, flags)?;
                pt.finish();
                #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                crate::mm::flush_tlb_on_all_cpus(pt.root_paddr(), Some(vaddr));
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
                // The old frame cannot be released until every CPU has
                // discarded a translation which may still reference it.
                pt.finish();
                #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                crate::mm::flush_tlb_on_all_cpus(pt.root_paddr(), Some(vaddr));
                drop_referenced_frame(paddr, self.size);
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
        let mut removed = Vec::new();
        let result = (|| {
            for addr in pages_in(range, self.size)? {
                match pt.unmap(addr) {
                    Ok((frame, _flags, page_size)) => {
                        assert_eq!(page_size, self.size);
                        removed.push(frame);
                    }
                    Err(PagingError::NotMapped) => {}
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(())
        })();

        finish_unmap_before_reclaim(pt);
        for frame in removed {
            drop_referenced_frame(frame, self.size);
            self.dec_mapped_pages();
        }
        result
    }

    fn on_protect(
        &self,
        _range: VirtAddrRange,
        new_flags: MappingFlags,
        _pt: &mut PageTableMut,
    ) -> AxResult<MappingFlags> {
        // The VMA records the requested WRITE permission, but an existing COW
        // PTE must remain read-only.  Its next write fault either upgrades a
        // uniquely owned frame or copies a shared frame before granting WRITE.
        Ok(new_flags - MappingFlags::WRITE)
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
        new_aspace: &Arc<AddrSpaceLock>,
    ) -> AxResult<Backend> {
        let new_backend = self.clone_for_fork(new_aspace);
        // Forking untouched lazy COW regions is common (e.g. fork14). In that
        // case, there is nothing in page tables to duplicate.
        if self.mapped_pages.load(Ordering::Relaxed) == 0 {
            return Ok(Backend::Cow(new_backend));
        }

        let cow_flags = flags - MappingFlags::WRITE;

        for vaddr in pages_in(range, self.size)? {
            // Registration above and this marker check close both sides of the
            // truncate snapshot race.  If truncate snapshotted before the
            // child listener was inserted, its marker is still visible and no
            // PTE is copied.  If it starts after this check, the snapshot
            // contains the child listener, whose callback waits for the child
            // address-space lock and revokes the copied PTE after area install.
            if self.file_page_change_in_progress(vaddr) {
                return Err(AxError::WouldBlock);
            }
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
            length_listener: Arc::new(Mutex::new(None)),
        })
    }

    pub fn new_alloc(start: VirtAddr, size: PageSize) -> Self {
        Self::Cow(CowBackend {
            start,
            size,
            file: None,
            mapped_pages: Arc::new(AtomicUsize::new(0)),
            identity: Arc::new(()),
            length_listener: Arc::new(Mutex::new(None)),
        })
    }
}
