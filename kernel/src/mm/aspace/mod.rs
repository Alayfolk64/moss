use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::{
    fmt, mem,
    ops::DerefMut,
    sync::atomic::{AtomicUsize, Ordering},
};

use axerrno::{AxError, AxResult, ax_bail};
use axhal::{
    mem::phys_to_virt,
    paging::{MappingFlags, PageTable, PagingError},
    trap::PageFaultFlags,
};
use axtask::yield_now;
use event_listener::{Event, listener};
use kspin::SpinNoIrq;
use memory_addr::{
    MemoryAddr, PAGE_SIZE_4K, PageIter4K, PhysAddr, VirtAddr, VirtAddrRange, is_aligned_4k,
};
use memory_set::{MappingError, MemoryArea, MemorySet, ProtectStats};

pub(crate) use backend::init_frame_ref_table;

const WRITER_LOCKED: usize = 1;
const ONE_READER: usize = 2;
// Keep the active owner/readers and the number of queued writers in one
// atomic word. A try-writer can therefore succeed only from the exact zero
// state and cannot jump ahead of a writer that has already queued.
const ONE_WAITING_WRITER: usize = 1usize << (usize::BITS / 2);
const ACTIVE_MASK: usize = ONE_WAITING_WRITER - 1;
const WAITING_WRITERS_MASK: usize = !ACTIVE_MASK;
static NEXT_ASPACE_ID: AtomicUsize = AtomicUsize::new(1);

pub(crate) struct RawAddrSpaceRwLock {
    state: AtomicUsize,
    event: Event,
}

impl RawAddrSpaceRwLock {
    const fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            event: Event::new(),
        }
    }

    fn try_lock_queued_writer(&self) -> bool {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            debug_assert_ne!(state & WAITING_WRITERS_MASK, 0);
            if state & ACTIVE_MASK != 0 {
                return false;
            }
            let next = (state - ONE_WAITING_WRITER) | WRITER_LOCKED;
            match self.state.compare_exchange_weak(
                state,
                next,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => state = observed,
            }
        }
    }
}

unsafe impl lock_api::RawRwLock for RawAddrSpaceRwLock {
    const INIT: Self = Self::new();
    type GuardMarker = lock_api::GuardSend;

    fn lock_shared(&self) {
        loop {
            if self.try_lock_shared() {
                return;
            }
            listener!(self.event => waiter);
            if self.try_lock_shared() {
                return;
            }
            axtask::future::block_on(waiter);
        }
    }

    fn try_lock_shared(&self) -> bool {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & (WRITER_LOCKED | WAITING_WRITERS_MASK) != 0 {
                return false;
            }
            debug_assert!(state <= ACTIVE_MASK - ONE_READER);
            match self.state.compare_exchange_weak(
                state,
                state + ONE_READER,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(next) => state = next,
            }
        }
    }

    unsafe fn unlock_shared(&self) {
        let previous = self.state.fetch_sub(ONE_READER, Ordering::Release);
        debug_assert!(previous & ACTIVE_MASK >= ONE_READER && previous & WRITER_LOCKED == 0);
        if previous & ACTIVE_MASK == ONE_READER {
            self.event.notify(usize::MAX);
        }
    }

    fn lock_exclusive(&self) {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                state.checked_add(ONE_WAITING_WRITER)
            })
            .expect("too many queued address-space writers");
        loop {
            if self.try_lock_queued_writer() {
                return;
            }
            listener!(self.event => waiter);
            if self.try_lock_queued_writer() {
                return;
            }
            axtask::future::block_on(waiter);
        }
    }

    fn try_lock_exclusive(&self) -> bool {
        self.state
            .compare_exchange(0, WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    unsafe fn unlock_exclusive(&self) {
        let previous = self.state.fetch_and(!WRITER_LOCKED, Ordering::Release);
        debug_assert_ne!(previous & WRITER_LOCKED, 0);
        debug_assert_eq!(previous & (ACTIVE_MASK - WRITER_LOCKED), 0);
        self.event.notify(usize::MAX);
    }
}

pub(crate) type BlockingRwLock<T> = lock_api::RwLock<RawAddrSpaceRwLock, T>;
pub(crate) type BlockingRwLockReadGuard<'a, T> =
    lock_api::RwLockReadGuard<'a, RawAddrSpaceRwLock, T>;
pub(crate) type BlockingRwLockWriteGuard<'a, T> =
    lock_api::RwLockWriteGuard<'a, RawAddrSpaceRwLock, T>;

mod backend;

pub use self::backend::*;

const MAX_CONCURRENT_FAULT_RETRIES: usize = 4;

#[inline]
fn synchronize_executable_fault(access_flags: PageFaultFlags) {
    if access_flags.contains(MappingFlags::EXECUTE) {
        // File contents are copied into a freshly allocated frame while the
        // address-space lock is released. Synchronize instruction fetch on the
        // hart which resolved the executable fault before retrying user code.
        #[cfg(target_arch = "riscv64")]
        riscv::asm::fence_i();
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            core::arch::asm!("ibar 0", options(nostack));
        }
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    let _ = access_flags;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VmaMetadata {
    user_flags: MappingFlags,
    may_flags: MappingFlags,
    shared: bool,
    file_backed: bool,
    anonymous: bool,
    locked: bool,
}

impl VmaMetadata {
    fn access_flags() -> MappingFlags {
        MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE
    }

    fn unrestricted(user_flags: MappingFlags, backend: &Backend) -> Self {
        let (shared, file_backed, anonymous) = backend.default_vma_kind();
        Self {
            user_flags: user_flags & Self::access_flags(),
            may_flags: Self::access_flags(),
            shared,
            file_backed,
            anonymous,
            locked: false,
        }
    }

    pub(crate) fn mmap(
        user_flags: MappingFlags,
        may_flags: MappingFlags,
        shared: bool,
        file_backed: bool,
        anonymous: bool,
        locked: bool,
    ) -> Self {
        Self {
            user_flags: user_flags & Self::access_flags(),
            may_flags: may_flags & Self::access_flags(),
            shared,
            file_backed,
            anonymous,
            locked,
        }
    }

    pub(crate) const fn shared(&self) -> bool {
        self.shared
    }
    pub(crate) const fn file_backed(&self) -> bool {
        self.file_backed
    }
    pub(crate) const fn anonymous(&self) -> bool {
        self.anonymous
    }
    pub(crate) const fn locked(&self) -> bool {
        self.locked
    }
    pub(crate) const fn user_flags(&self) -> MappingFlags {
        self.user_flags
    }

    fn permits(&self, flags: MappingFlags) -> bool {
        self.may_flags.contains(flags & Self::access_flags())
    }

    fn with_user_flags(mut self, flags: MappingFlags) -> Self {
        self.user_flags = flags & Self::access_flags();
        self
    }

    fn for_fork(mut self) -> Self {
        self.locked = false;
        self
    }
}

/// The virtual memory address space.
pub struct AddrSpace {
    va_range: VirtAddrRange,
    areas: MemorySet<Backend, VmaMetadata>,
    pt: PageTable,
}

/// Synchronizes structural address-space changes while allowing unrelated
/// page faults to inspect and update independent PTEs concurrently.
pub struct AddrSpaceLock {
    inner: BlockingRwLock<AddrSpace>,
    root_paddr: PhysAddr,
    id: usize,
}

impl AddrSpaceLock {
    pub(crate) fn new(aspace: AddrSpace) -> Self {
        let root_paddr = aspace.page_table_root();
        Self {
            inner: BlockingRwLock::new(aspace),
            root_paddr,
            id: NEXT_ASPACE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Returns the immutable page-table identity without taking the
    /// address-space contents lock. This is also safe from scheduler hooks
    /// which run with local interrupts disabled.
    pub(crate) const fn page_table_root(&self) -> PhysAddr {
        self.root_paddr
    }

    pub(crate) const fn eviction_owner(&self) -> usize {
        self.id
    }

    /// Existing mapping-changing operations take the exclusive side.
    pub(crate) fn lock(&self) -> BlockingRwLockWriteGuard<'_, AddrSpace> {
        self.inner.write()
    }

    /// Page faults take the shared side only for VMA lookup and PTE commit.
    pub(crate) fn read(&self) -> BlockingRwLockReadGuard<'_, AddrSpace> {
        self.inner.read()
    }

    pub(crate) fn try_lock(&self) -> Option<BlockingRwLockWriteGuard<'_, AddrSpace>> {
        self.inner.try_write()
    }
}

static RECLAIM_QUEUE: SpinNoIrq<VecDeque<Arc<AddrSpaceLock>>> = SpinNoIrq::new(VecDeque::new());
static RECLAIM_WORKERS: AtomicUsize = AtomicUsize::new(0);

fn start_reclaim_worker() {
    loop {
        let workers = RECLAIM_WORKERS.load(Ordering::Acquire);
        if workers >= axhal::cpu_num() || RECLAIM_QUEUE.lock().is_empty() {
            return;
        }
        if RECLAIM_WORKERS
            .compare_exchange_weak(workers, workers + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }

        axtask::spawn(|| {
            loop {
                let Some(aspace) = RECLAIM_QUEUE.lock().pop_front() else {
                    RECLAIM_WORKERS.fetch_sub(1, Ordering::AcqRel);
                    // An enqueue can race with the empty observation while all
                    // workers are leaving. Recheck after publishing the new
                    // worker count so that queued work cannot be stranded.
                    start_reclaim_worker();
                    return;
                };
                aspace.lock().clear();
            }
        });
        return;
    }
}

/// Clears a dead address space outside the exit or execve critical path.
pub(crate) fn defer_address_space_reclaim(aspace: Arc<AddrSpaceLock>) {
    RECLAIM_QUEUE.lock().push_back(aspace);
    start_reclaim_worker();
}

impl AddrSpace {
    /// Returns the address space base.
    pub const fn base(&self) -> VirtAddr {
        self.va_range.start
    }

    /// Returns the address space end.
    pub const fn end(&self) -> VirtAddr {
        self.va_range.end
    }

    /// Returns the address space size.
    pub fn size(&self) -> usize {
        self.va_range.size()
    }

    /// Returns the reference to the inner page table.
    pub const fn page_table(&self) -> &PageTable {
        &self.pt
    }

    /// Returns a mutable reference to the inner page table.
    pub const fn page_table_mut(&mut self) -> &mut PageTable {
        &mut self.pt
    }

    /// Returns the root physical address of the inner page table.
    pub const fn page_table_root(&self) -> PhysAddr {
        self.pt.root_paddr()
    }

    /// Checks if the address space contains the given address range.
    pub fn contains_range(&self, start: VirtAddr, size: usize) -> bool {
        self.va_range.contains(start) && (self.va_range.end - start) >= size
    }

    /// Creates a new empty address space.
    pub fn new_empty(base: VirtAddr, size: usize) -> AxResult<Self> {
        Ok(Self {
            va_range: VirtAddrRange::from_start_size(base, size),
            areas: MemorySet::new(),
            pt: PageTable::try_new().map_err(|_| AxError::NoMemory)?,
        })
    }

    fn validate_region(&self, start: VirtAddr, size: usize) -> AxResult {
        if !self.contains_range(start, size) {
            ax_bail!(NoMemory, "address out of range");
        }
        if !start.is_aligned_4k() || !is_aligned_4k(size) {
            ax_bail!(InvalidInput, "address is not aligned");
        }
        Ok(())
    }

    /// Finds a free area that can accommodate the given size.
    ///
    /// The search starts from the given hint address, and the area should be
    /// within the given limit range.
    ///
    /// Returns the start address of the free area. Returns None if no such area
    /// is found.
    pub fn find_free_area(
        &self,
        hint: VirtAddr,
        size: usize,
        limit: VirtAddrRange,
        align: usize,
    ) -> Option<VirtAddr> {
        self.areas.find_free_area(hint, size, limit, align)
    }

    pub(crate) fn find_area(&self, vaddr: VirtAddr) -> Option<&MemoryArea<Backend, VmaMetadata>> {
        self.areas.find(vaddr)
    }

    /// Add a new linear mapping.
    ///
    /// See [`Backend`] for more details about the mapping backends.
    ///
    /// The `flags` parameter indicates the mapping permissions and attributes.
    ///
    /// Returns an error if the address range is out of the address space or not
    /// aligned.
    pub fn map_linear(
        &mut self,
        start_vaddr: VirtAddr,
        start_paddr: PhysAddr,
        size: usize,
        flags: MappingFlags,
    ) -> AxResult {
        self.validate_region(start_vaddr, size)?;

        if !start_paddr.is_aligned_4k() {
            ax_bail!(InvalidInput, "address is not aligned");
        }

        let offset = start_vaddr.as_usize() as isize - start_paddr.as_usize() as isize;
        let backend = Backend::new_linear(offset);
        let metadata = VmaMetadata::unrestricted(flags, &backend);
        let area = MemoryArea::new_with_metadata(start_vaddr, size, flags, backend, metadata);
        self.areas.map(area, &mut self.pt, false)?;
        Ok(())
    }

    // 增加一块内存区域
    pub fn map(
        &mut self,
        start: VirtAddr,
        size: usize,
        flags: MappingFlags,
        populate: bool,
        backend: Backend,
    ) -> AxResult {
        let metadata = VmaMetadata::unrestricted(flags, &backend);
        self.map_with_metadata(start, size, flags, populate, backend, metadata)
    }

    pub(crate) fn map_with_metadata(
        &mut self,
        start: VirtAddr,
        size: usize,
        flags: MappingFlags,
        populate: bool,
        backend: Backend,
        metadata: VmaMetadata,
    ) -> AxResult {
        self.validate_region(start, size)?;

        let area = MemoryArea::new_with_metadata(start, size, flags, backend, metadata);
        self.areas.map(area, &mut self.pt, false)?;
        if populate {
            if let Err(err) = self.populate_area(start, size, flags) {
                // `MemorySet::map` has already published the VMA, and a
                // backend may have populated a prefix before reporting an
                // error.  A failed mmap must leave neither the area nor those
                // PTE/frame/listener side effects behind.
                self.areas
                    .unmap(start, size, &mut self.pt)
                    .expect("failed to roll back partially populated mapping");
                return Err(err);
            }
        }
        Ok(())
    }

    /// Populates the area with physical frames, returning false if the area
    /// contains unmapped area.
    /// 真正分配物理页
    pub fn populate_area(
        &mut self,
        mut start: VirtAddr,
        size: usize,
        access_flags: MappingFlags,
    ) -> AxResult {
        self.validate_region(start, size)?;
        let end = start + size;

        let mut callbacks = Vec::new();
        let populate_result: AxResult = {
            let mut modify = self.pt.modify();
            (|| {
                while let Some(area) = self.areas.find(start) {
                    let range = VirtAddrRange::new(start, area.end().min(end));
                    let (_, callback) =
                        area.backend()
                            .populate(range, area.flags(), access_flags, &mut modify)?;
                    if let Some(callback) = callback {
                        callbacks.push(callback);
                    }
                    start = area.end();
                    assert!(start.is_aligned_4k());
                    if start >= end {
                        break;
                    }
                }
                Ok(())
            })()
        };
        for callback in callbacks {
            callback(self);
        }
        populate_result?;

        if start < end {
            // If the area is not fully mapped, we return ENOMEM.
            ax_bail!(NoMemory);
        }

        Ok(())
    }

    /// Removes mappings within the specified virtual address range.
    ///
    /// Returns an error if the address range is out of the address space or not
    /// aligned.
    /// 删除一块区域
    pub fn unmap(&mut self, start: VirtAddr, size: usize) -> AxResult {
        self.validate_region(start, size)?;

        self.areas.unmap(start, size, &mut self.pt)?;
        Ok(())
    }

    pub fn zap_pages(&mut self, start: VirtAddr, size: usize) -> AxResult {
        self.validate_region(start, size)?;
        let end = start + size;
        let mut modify = self.pt.modify();
        for vaddr in PageIter4K::new(start, end).ok_or(AxError::InvalidInput)? {
            match modify.unmap(vaddr) {
                Ok(_) | Err(PagingError::NotMapped) => {}
                Err(_) => return Err(AxError::BadAddress),
            }
        }
        Ok(())
    }

    /// Drops resident pages in a mapped range without removing its memory
    /// areas. Later accesses fault the pages back in from zero or their file.
    pub fn discard_pages(&mut self, mut start: VirtAddr, size: usize) -> AxResult {
        self.validate_region(start, size)?;
        let end = start + size;
        let mut modify = self.pt.modify();

        while start < end {
            let area = self.areas.find(start).ok_or(AxError::NoMemory)?;
            let range = VirtAddrRange::new(start, area.end().min(end));
            area.backend().discard(range, &mut modify)?;
            start = range.end;
        }
        Ok(())
    }

    /// To process data in this area with the given function.
    ///
    /// Now it supports reading and writing data in the given interval.
    fn process_area_data<F>(&self, start: VirtAddr, size: usize, mut f: F) -> AxResult
    where
        F: FnMut(VirtAddr, usize, usize),
    {
        if !self.contains_range(start, size) {
            ax_bail!(InvalidInput, "address out of range");
        }
        let mut cnt = 0;
        // If start is aligned to 4K, start_align_down will be equal to start_align_up.
        let end_align_up = (start + size).align_up_4k();
        for vaddr in PageIter4K::new(start.align_down_4k(), end_align_up)
            .expect("Failed to create page iterator")
        {
            let (mut paddr, ..) = self.pt.query(vaddr).map_err(|_| AxError::BadAddress)?;

            let mut copy_size = (size - cnt).min(PAGE_SIZE_4K);

            if copy_size == 0 {
                break;
            }
            if vaddr == start.align_down_4k() && start.align_offset_4k() != 0 {
                let align_offset = start.align_offset_4k();
                copy_size = copy_size.min(PAGE_SIZE_4K - align_offset);
                paddr += align_offset;
            }
            f(phys_to_virt(paddr), cnt, copy_size);
            cnt += copy_size;
        }
        Ok(())
    }

    /// To read data from the address space.
    ///
    /// # Arguments
    ///
    /// * `start` - The start virtual address to read.
    /// * `buf` - The buffer to store the data.
    pub fn read(&self, start: VirtAddr, buf: &mut [u8]) -> AxResult {
        self.process_area_data(start, buf.len(), |src, offset, read_size| unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), buf.as_mut_ptr().add(offset), read_size);
        })
    }

    /// To write data to the address space.
    ///
    /// # Arguments
    ///
    /// * `start_vaddr` - The start virtual address to write.
    /// * `buf` - The buffer to write to the address space.
    pub fn write(&self, start: VirtAddr, buf: &[u8]) -> AxResult {
        self.process_area_data(start, buf.len(), |dst, offset, write_size| unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr().add(offset), dst.as_mut_ptr(), write_size);
        })
    }

    /// Updates mapping within the specified virtual address range.
    ///
    /// Returns an error if the address range is out of the address space or not
    /// aligned.
    /// 改权限，比如只读改成可读写
    pub fn protect(
        &mut self,
        start: VirtAddr,
        size: usize,
        flags: MappingFlags,
        user_flags: MappingFlags,
    ) -> AxResult<ProtectStats> {
        self.validate_region(start, size)?;

        let access_flags = VmaMetadata::access_flags();
        let requested = flags & access_flags;
        let requested_user = user_flags & access_flags;
        Ok(self.areas.protect(
            start,
            size,
            |area| {
                if !area.metadata().permits(requested_user) {
                    return Err(MappingError::PermissionDenied);
                }
                let new_flags = (area.flags() - access_flags) | requested;
                let new_metadata = area.metadata().with_user_flags(requested_user);
                Ok(
                    (new_flags != area.flags() || new_metadata != *area.metadata())
                        .then_some((new_flags, new_metadata)),
                )
            },
            |left, right| {
                left.flags() == right.flags()
                    && left.metadata() == right.metadata()
                    && left.backend().merge_compatible(right.backend())
            },
            &mut self.pt,
        )?)
    }

    pub(crate) fn locked_bytes_in_range(&self, start: VirtAddr, size: usize) -> usize {
        let Some(end) = start.checked_add(size) else {
            return 0;
        };
        self.areas
            .iter()
            .filter(|area| area.metadata().locked())
            .map(|area| {
                area.end()
                    .min(end)
                    .as_usize()
                    .saturating_sub(area.start().max(start).as_usize())
            })
            .sum()
    }

    pub(crate) fn locked_bytes(&self) -> usize {
        self.areas
            .iter()
            .filter(|area| area.metadata().locked())
            .map(|area| area.size())
            .sum()
    }

    pub(crate) fn range_is_private_anonymous(&self, start: VirtAddr, size: usize) -> bool {
        self.range_metadata_matches(start, size, |metadata| {
            !metadata.shared() && metadata.anonymous()
        })
    }

    pub(crate) fn range_is_shared_file(&self, start: VirtAddr, size: usize) -> bool {
        self.range_metadata_matches(start, size, |metadata| {
            metadata.shared() && metadata.file_backed()
        })
    }

    fn range_metadata_matches(
        &self,
        start: VirtAddr,
        size: usize,
        predicate: impl Fn(&VmaMetadata) -> bool,
    ) -> bool {
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        let mut cursor = start;
        while cursor < end {
            let Some(area) = self.areas.find(cursor) else {
                return false;
            };
            if !predicate(area.metadata()) {
                return false;
            }
            cursor = area.end().min(end);
        }
        true
    }

    /// Removes all mappings in the address space.
    pub fn clear(&mut self) {
        // Retain all backend owners while one page-table cursor and one
        // deferred-frame gather remove every VMA. This turns per-VMA remote
        // shootdowns into shared bounded batches.
        let areas = mem::take(&mut self.areas);
        let mut pt = self.pt.modify();
        let mut gather = UnmapGather::new();
        let mut result = Ok(());
        for area in areas.iter() {
            if let Err(err) = area
                .backend()
                .unmap_for_clear(area.va_range(), &mut pt, &mut gather)
            {
                result = Err(err);
                break;
            }
        }
        gather.finish(&mut pt);
        pt.finish();
        drop(pt);
        match result {
            Ok(()) => drop(areas),
            Err(err) => {
                // Preserve backend owners on an impossible metadata/PTE
                // disagreement; freeing them could recycle a still-mapped
                // frame after the partial clear.
                mem::forget(areas);
                panic!("failed to clear address space: {err}");
            }
        }
    }

    /// Checks whether an access to the specified memory region is valid.
    ///
    /// Returns `true` if the memory region given by `range` is all mapped and
    /// has proper permission flags (i.e. containing `access_flags`).
    pub fn can_access_range(
        &self,
        start: VirtAddr,
        size: usize,
        access_flags: MappingFlags,
    ) -> bool {
        let Some(mut range) = VirtAddrRange::try_from_start_size(start, size) else {
            return false;
        };
        for area in self.areas.iter() {
            if area.end() <= range.start {
                continue;
            }
            if area.start() > range.start {
                return false;
            }

            // This area overlaps with the memory region
            if !area.flags().contains(access_flags) {
                return false;
            }

            range.start = area.end();
            if range.is_empty() {
                return true;
            }
        }

        false
    }

    /// Returns an approximate resident set size in bytes for user mappings.
    ///
    /// Lazy mmap/brk areas are only counted after their pages have actually
    /// been populated into the page table, matching the RSS semantics needed by
    /// `/proc/*/status` and `getrusage(2)`.
    pub fn resident_set_size(&self) -> usize {
        let mut bytes = 0;
        for area in self.areas.iter() {
            if !area.flags().contains(MappingFlags::USER) {
                continue;
            }
            let Some(pages) = PageIter4K::new(area.start(), area.end()) else {
                continue;
            };
            for vaddr in pages {
                if self.pt.query(vaddr).is_ok() {
                    bytes += PAGE_SIZE_4K;
                }
            }
        }
        bytes
    }

    pub fn is_accessible_fault(&self, vaddr: VirtAddr, access_flags: MappingFlags) -> bool {
        self.areas
            .find(vaddr)
            .is_some_and(|area| area.flags().contains(access_flags))
    }

    /// Handles a page fault at the given address.
    ///
    /// `access_flags` indicates the access type that caused the page fault.
    ///
    /// Returns `true` if the page fault is handled successfully (not a real
    /// fault).
    pub fn handle_page_fault(&mut self, vaddr: VirtAddr, access_flags: PageFaultFlags) -> bool {
        match self.try_handle_page_fault(vaddr, access_flags) {
            Ok(handled) => handled,
            Err(err) => {
                warn!("Failed to populate page for {vaddr:?}: {err}");
                false
            }
        }
    }

    fn try_handle_page_fault(
        &mut self,
        vaddr: VirtAddr,
        access_flags: PageFaultFlags,
    ) -> AxResult<bool> {
        if !self.va_range.contains(vaddr) {
            return Ok(false);
        }
        if let Some(area) = self.areas.find(vaddr) {
            let flags = area.flags();
            if flags.contains(access_flags) {
                let page_size = area.backend().page_size();
                let populate_result = area.backend().populate(
                    VirtAddrRange::from_start_size(vaddr.align_down(page_size), page_size as _),
                    flags,
                    access_flags,
                    &mut self.pt.modify(),
                );
                return match populate_result {
                    Ok((n, callback)) => {
                        if let Some(cb) = callback {
                            cb(self);
                        }
                        if n == 0 {
                            warn!("No pages populated for {vaddr:?} ({flags:?})");
                            Ok(false)
                        } else {
                            // Another CPU may have installed this mapping while
                            // this CPU was waiting for the address-space lock.
                            // Drop any locally cached invalid translation before
                            // retrying the faulting instruction.
                            axhal::asm::flush_tlb(Some(vaddr));
                            synchronize_executable_fault(access_flags);
                            Ok(true)
                        }
                    }
                    Err(AxError::WouldBlock) => Err(AxError::WouldBlock),
                    Err(err) => {
                        warn!("Failed to populate pages for {vaddr:?} ({flags:?}): {err}");
                        Ok(false)
                    }
                };
            }
        }
        Ok(false)
    }

    /// Handles a private fault in three phases: inspect while read-locked,
    /// allocate/copy/read without the address-space lock, then revalidate and
    /// atomically publish the PTE while read-locked again.
    pub fn handle_page_fault_concurrent(
        aspace: &Arc<AddrSpaceLock>,
        vaddr: VirtAddr,
        access_flags: PageFaultFlags,
    ) -> bool {
        Self::handle_page_fault_concurrent_inner(aspace, vaddr, access_flags, true)
    }

    /// Populates one page for a kernel access check. Unlike a real user fault,
    /// observing that another thread already installed the mapping does not
    /// require a local TLB invalidation.
    pub fn populate_page_concurrent(
        aspace: &Arc<AddrSpaceLock>,
        vaddr: VirtAddr,
        access_flags: MappingFlags,
    ) -> bool {
        Self::handle_page_fault_concurrent_inner(aspace, vaddr, access_flags, false)
    }

    fn handle_page_fault_concurrent_inner(
        aspace: &Arc<AddrSpaceLock>,
        vaddr: VirtAddr,
        access_flags: PageFaultFlags,
        flush_existing: bool,
    ) -> bool {
        let mut stale_retries = 0;
        loop {
            let preparation = {
                let guard = aspace.read();
                if !guard.va_range.contains(vaddr) {
                    return false;
                }
                let Some(area) = guard.areas.find(vaddr) else {
                    return false;
                };
                if !area.flags().contains(access_flags) {
                    return false;
                }
                area.backend().prepare_fault(
                    aspace,
                    vaddr,
                    area.va_range(),
                    area.flags(),
                    access_flags,
                    &guard.pt,
                )
            };

            let committed = match preparation {
                FaultPreparation::Handled => {
                    if flush_existing {
                        axhal::asm::flush_tlb(Some(vaddr));
                        synchronize_executable_fault(access_flags);
                    }
                    return true;
                }
                FaultPreparation::Retry => {
                    // The read guard used for preparation has been dropped.
                    // Let the eviction owner finish its PTE/TLB/writeback
                    // barrier before taking the exclusive fallback lock.
                    yield_now();
                    continue;
                }
                FaultPreparation::Fallback => break,
                FaultPreparation::Cow(mut plan) => {
                    match plan.execute() {
                        Ok(()) => {}
                        Err(AxError::WouldBlock) => {
                            // A file truncate published its affected range after
                            // this fault sampled the old EOF. Retry only after
                            // dropping the address-space read lock (already done
                            // above), so truncate can finish its PTE/TLB barrier.
                            yield_now();
                            continue;
                        }
                        Err(err) => {
                            warn!("Failed to prepare private page for {vaddr:?}: {err}");
                            return false;
                        }
                    }

                    let guard = aspace.read();
                    let Self { areas, pt, .. } = &*guard;
                    let Some(area) = areas.find(vaddr) else {
                        return false;
                    };
                    if !area.flags().contains(access_flags) {
                        return false;
                    }
                    let flags = area.flags();
                    area.backend()
                        .commit_fault(&mut plan, area.va_range(), flags, access_flags, pt)
                }
                FaultPreparation::File(mut plan) => {
                    if let Err(err) = plan.execute() {
                        warn!("Failed to prepare file page for {vaddr:?}: {err}");
                        return false;
                    }

                    let guard = aspace.read();
                    let Self { areas, pt, .. } = &*guard;
                    let Some(area) = areas.find(vaddr) else {
                        return false;
                    };
                    if !area.flags().contains(access_flags) {
                        return false;
                    }
                    let flags = area.flags();
                    area.backend().commit_file_fault(
                        &mut plan,
                        area.va_range(),
                        flags,
                        access_flags,
                        pt,
                    )
                }
                FaultPreparation::Shared(mut plan) => {
                    if let Err(err) = plan.execute() {
                        warn!("Failed to prepare shared page for {vaddr:?}: {err}");
                        return false;
                    }

                    let guard = aspace.read();
                    let Self { areas, pt, .. } = &*guard;
                    let Some(area) = areas.find(vaddr) else {
                        return false;
                    };
                    if !area.flags().contains(access_flags) {
                        return false;
                    }
                    let flags = area.flags();
                    area.backend().commit_shared_fault(
                        &mut plan,
                        area.va_range(),
                        flags,
                        access_flags,
                        pt,
                    )
                }
            };

            match committed {
                Ok(FaultCommit::Handled) => {
                    axhal::asm::flush_tlb(Some(vaddr));
                    synchronize_executable_fault(access_flags);
                    return true;
                }
                Ok(FaultCommit::Stale) if stale_retries < MAX_CONCURRENT_FAULT_RETRIES => {
                    stale_retries += 1;
                }
                Ok(FaultCommit::Stale) => break,
                Err(err) => {
                    warn!("Failed to commit page for {vaddr:?}: {err}");
                    return false;
                }
            }
        }

        loop {
            let result = aspace.lock().try_handle_page_fault(vaddr, access_flags);
            match result {
                Ok(handled) => return handled,
                Err(AxError::WouldBlock) => {
                    // `populate` rechecked the cache after taking the exclusive
                    // address-space lock and raced with a sync detach.  Drop the
                    // lock (above), yield, and restart preparation.
                    yield_now();
                }
                Err(err) => {
                    warn!("Failed to populate page for {vaddr:?}: {err}");
                    return false;
                }
            }
        }
    }

    /// Attempts to clone the current address space into a new one.
    ///
    /// This method creates a new empty address space with the same base and
    /// size, then iterates over all memory areas in the original address
    /// space to copy or share their mappings into the new one.
    pub fn try_clone(&mut self) -> AxResult<Arc<AddrSpaceLock>> {
        let new_aspace = Arc::new(AddrSpaceLock::new(Self::new_empty(
            self.base(),
            self.size(),
        )?));
        let new_aspace_clone = new_aspace.clone();

        let mut guard = new_aspace.lock();

        let mut self_modify = self.pt.modify();
        for area in self.areas.iter() {
            let new_backend = area.backend().clone_map(
                area.va_range(),
                area.flags(),
                &mut self_modify,
                &mut guard.pt.modify(),
                &new_aspace_clone,
            )?;

            let new_area = MemoryArea::new_with_metadata(
                area.start(),
                area.size(),
                area.flags(),
                new_backend,
                area.metadata().for_fork(),
            );
            let aspace = guard.deref_mut();
            aspace.areas.map(new_area, &mut aspace.pt, false)?;
        }
        drop(guard);

        Ok(new_aspace)
    }

    /// Returns an iterator over the memory areas.
    ///
    /// This is required for `procfs` to generate `/proc/pid/maps`.
    /// Exposing internal state for system introspection is a standard practice.
    pub(crate) fn areas(&self) -> impl Iterator<Item = &MemoryArea<Backend, VmaMetadata>> {
        self.areas.iter()
    }
}

impl fmt::Debug for AddrSpace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("AddrSpace")
            .field("va_range", &self.va_range)
            .field("page_table_root", &self.pt.root_paddr())
            .field("areas", &self.areas)
            .finish()
    }
}

impl Drop for AddrSpace {
    fn drop(&mut self) {
        self.clear();
    }
}
