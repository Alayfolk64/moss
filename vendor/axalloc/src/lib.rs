//! [ArceOS](https://github.com/arceos-org/arceos) global memory allocator.
//!
//! It provides [`GlobalAllocator`], which implements the trait
//! [`core::alloc::GlobalAlloc`]. A static global variable of type
//! [`GlobalAllocator`] is defined with the `#[global_allocator]` attribute, to
//! be registered as the standard library’s default allocator.

#![no_std]

#[macro_use]
extern crate log;
extern crate alloc;

use core::{
    alloc::{GlobalAlloc, Layout},
    fmt,
    ptr::NonNull,
    sync::atomic::{AtomicUsize, Ordering},
};

#[allow(unused_imports)]
use axallocator::{AllocResult, BaseAllocator, BitmapPageAllocator, ByteAllocator, PageAllocator};
use kspin::{NoPreemptIrqSave, SpinIrq, SpinNoIrq};
use strum::{IntoStaticStr, VariantArray};

const PAGE_SIZE: usize = 0x1000;
const MIN_HEAP_SIZE: usize = 0x8000; // 32 K
const MAX_HEAP_EXPAND_SIZE: usize = 16 * 1024 * 1024;

mod page;
pub use page::GlobalPage;

mod pcp;
use pcp::{PCP_CPU_SLOTS, PerCpuPageCache};

#[cfg(feature = "tracking")]
mod tracking;
#[cfg(feature = "tracking")]
pub use tracking::*;

cfg_if::cfg_if! {
    if #[cfg(feature = "slab")] {
        /// The default byte allocator.
        pub type DefaultByteAllocator = axallocator::SlabByteAllocator;
    } else if #[cfg(feature = "buddy")] {
        /// The default byte allocator.
        pub type DefaultByteAllocator = axallocator::BuddyByteAllocator;
    } else if #[cfg(feature = "tlsf")] {
        /// The default byte allocator.
        pub type DefaultByteAllocator = axallocator::TlsfByteAllocator;
    }
}

/// Kinds of memory usage for tracking.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, VariantArray, IntoStaticStr)]
pub enum UsageKind {
    /// Heap allocations made by kernel Rust code.
    RustHeap,
    /// Virtual memory, usually used for user space.
    VirtMem,
    /// Page cache for file systems.
    PageCache,
    /// Page tables.
    PageTable,
    /// DMA memory.
    Dma,
    /// Memory used by [`GlobalPage`].
    Global,
}

/// Statistics of memory usages.
#[derive(Clone, Copy)]
pub struct Usages([usize; UsageKind::VARIANTS.len()]);

impl Usages {
    pub fn get(&self, kind: UsageKind) -> usize {
        self.0[kind as usize]
    }
}

/// Lock-free accounting counters.
///
/// These values are diagnostic statistics and do not publish allocator
/// ownership, so relaxed operations are sufficient. A snapshot may observe
/// different categories at slightly different instants.
struct AtomicUsages([AtomicUsize; UsageKind::VARIANTS.len()]);

impl AtomicUsages {
    const fn new() -> Self {
        Self([const { AtomicUsize::new(0) }; UsageKind::VARIANTS.len()])
    }

    fn alloc(&self, kind: UsageKind, size: usize) {
        self.0[kind as usize].fetch_add(size, Ordering::Relaxed);
    }

    fn dealloc(&self, kind: UsageKind, size: usize) {
        let old = self.0[kind as usize].fetch_sub(size, Ordering::Relaxed);
        debug_assert!(old >= size, "usage counter underflow");
    }

    fn snapshot(&self) -> Usages {
        Usages(core::array::from_fn(|index| {
            self.0[index].load(Ordering::Relaxed)
        }))
    }
}

impl fmt::Debug for Usages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("UsageStats");
        for &kind in UsageKind::VARIANTS {
            d.field(kind.into(), &self.0[kind as usize]);
        }
        d.finish()
    }
}

/// The global allocator used by ArceOS.
///
/// It combines a [`ByteAllocator`] and a [`PageAllocator`] into a simple
/// two-level allocator: firstly tries allocate from the byte allocator, if
/// there is no memory, asks the page allocator for more memory and adds it to
/// the byte allocator.
///
/// Currently, [`TlsfByteAllocator`] is used as the byte allocator, while
/// [`BitmapPageAllocator`] is used as the page allocator.
///
/// [`TlsfByteAllocator`]: axallocator::TlsfByteAllocator
pub struct GlobalAllocator {
    balloc: SpinNoIrq<DefaultByteAllocator>,
    #[cfg(not(feature = "level-1"))]
    palloc: SpinNoIrq<BitmapPageAllocator<PAGE_SIZE>>,
    page_caches: [SpinIrq<PerCpuPageCache>; PCP_CPU_SLOTS],
    page_cache_counts: [CacheAlignedCount; PCP_CPU_SLOTS],
    managed_pages: AtomicUsize,
    usages: AtomicUsages,
}

#[repr(align(64))]
struct CacheAlignedCount(AtomicUsize);

impl CacheAlignedCount {
    const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }
}

impl Default for GlobalAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalAllocator {
    /// Creates an empty [`GlobalAllocator`].
    pub const fn new() -> Self {
        Self {
            balloc: SpinNoIrq::new(DefaultByteAllocator::new()),
            #[cfg(not(feature = "level-1"))]
            palloc: SpinNoIrq::new(BitmapPageAllocator::new()),
            page_caches: [const { SpinIrq::new(PerCpuPageCache::new()) }; PCP_CPU_SLOTS],
            page_cache_counts: [const { CacheAlignedCount::new() }; PCP_CPU_SLOTS],
            managed_pages: AtomicUsize::new(0),
            usages: AtomicUsages::new(),
        }
    }

    /// Returns the name of the allocator.
    pub const fn name(&self) -> &'static str {
        cfg_if::cfg_if! {
            if #[cfg(feature = "slab")] {
                "slab"
            } else if #[cfg(feature = "buddy")] {
                "buddy"
            } else if #[cfg(feature = "tlsf")] {
                "TLSF"
            }
        }
    }

    /// Initializes the allocator with the given region.
    ///
    /// It firstly adds the whole region to the page allocator, then allocates
    /// a small region (32 KB) to initialize the byte allocator. Therefore,
    /// the given region must be larger than 32 KB.
    pub fn init(&self, start_vaddr: usize, size: usize) {
        assert!(size > MIN_HEAP_SIZE);
        #[cfg(not(feature = "level-1"))]
        {
            let init_heap_size = MIN_HEAP_SIZE;
            self.palloc.lock().init(start_vaddr, size);
            self.managed_pages
                .store(size / PAGE_SIZE, Ordering::Relaxed);
            let heap_ptr = self
                .alloc_pages(init_heap_size / PAGE_SIZE, PAGE_SIZE, UsageKind::RustHeap)
                .unwrap();

            self.balloc.lock().init(heap_ptr, init_heap_size);
        }
        #[cfg(feature = "level-1")]
        {
            self.balloc.lock().init(start_vaddr, size);
            self.managed_pages
                .store(size / PAGE_SIZE, Ordering::Relaxed);
        }
    }

    /// Add the given region to the allocator.
    ///
    /// In the two-level allocator, the page allocator owns every physical
    /// memory region and grows the byte allocator on demand. This prevents a
    /// secondary RAM bank from becoming byte-heap-only and therefore
    /// unavailable to user address spaces.
    pub fn add_memory(&self, start_vaddr: usize, size: usize) -> AllocResult {
        #[cfg(feature = "level-1")]
        let result = self.balloc.lock().add_memory(start_vaddr, size);
        #[cfg(not(feature = "level-1"))]
        let result = self.palloc.lock().add_memory(start_vaddr, size);
        if result.is_ok() {
            self.managed_pages
                .fetch_add(size / PAGE_SIZE, Ordering::Relaxed);
        }
        result
    }

    /// Allocate arbitrary number of bytes. Returns the left bound of the
    /// allocated region.
    ///
    /// It firstly tries to allocate from the byte allocator. If there is no
    /// memory, it asks the page allocator for more memory and adds it to the
    /// byte allocator.
    pub fn alloc(&self, layout: Layout) -> AllocResult<NonNull<u8>> {
        #[cfg(feature = "level-1")]
        {
            self.alloc_level1(layout)
        }
        #[cfg(not(feature = "level-1"))]
        {
            self.alloc_level2(layout)
        }
    }

    #[cfg(feature = "level-1")]
    fn alloc_level1(&self, layout: Layout) -> AllocResult<NonNull<u8>> {
        // single-level allocator: only use the byte allocator.
        let mut balloc = self.balloc.lock();
        let ptr = balloc.alloc(layout)?;
        drop(balloc);
        self.usages.alloc(UsageKind::RustHeap, layout.size());
        Ok(ptr)
    }

    #[cfg(not(feature = "level-1"))]
    fn alloc_level2(&self, layout: Layout) -> AllocResult<NonNull<u8>> {
        // simple two-level allocator: if no heap memory, allocate from the page allocator.
        let mut balloc = self.balloc.lock();
        loop {
            if let Ok(ptr) = balloc.alloc(layout) {
                drop(balloc);
                self.usages.alloc(UsageKind::RustHeap, layout.size());
                return Ok(ptr);
            } else {
                let old_size = balloc.total_bytes();
                // A buddy-backed byte allocator rounds large allocations up
                // to a power of two and requires the resulting block to have
                // the same alignment.  Grow the byte heap with that invariant
                // instead of handing it an exactly-sized, merely page-aligned
                // region that it can never use for `layout`.
                let required_span = layout.size().max(layout.align());
                // Doubling the whole byte heap can reserve hundreds of MiB
                // from the page allocator while the Rust heap has only a
                // fraction of that memory live. Bound ordinary growth so a
                // 1-GiB machine retains pages for user address spaces.
                let expand_size = old_size
                    .min(MAX_HEAP_EXPAND_SIZE)
                    .max(required_span)
                    .next_power_of_two()
                    .max(PAGE_SIZE);

                let mut try_size = expand_size;
                let min_size = PAGE_SIZE.max(required_span);
                loop {
                    let heap_ptr =
                        match self.alloc_pages(try_size / PAGE_SIZE, try_size, UsageKind::RustHeap)
                        {
                            Ok(ptr) => ptr,
                            Err(err) => {
                                try_size /= 2;
                                if try_size < min_size {
                                    return Err(err);
                                }
                                continue;
                            }
                        };
                    debug!(
                        "expand heap memory: [{:#x}, {:#x})",
                        heap_ptr,
                        heap_ptr + try_size
                    );
                    balloc.add_memory(heap_ptr, try_size)?;
                    break;
                }
            }
        }
    }

    /// Gives back the allocated region to the byte allocator.
    ///
    /// The region should be allocated by [`alloc`], and `align_pow2` should be
    /// the same as the one used in [`alloc`]. Otherwise, the behavior is
    /// undefined.
    ///
    /// [`alloc`]: GlobalAllocator::alloc
    pub fn dealloc(&self, pos: NonNull<u8>, layout: Layout) {
        self.usages.dealloc(UsageKind::RustHeap, layout.size());
        let released = {
            let mut balloc = self.balloc.lock();
            balloc.dealloc(pos, layout);
            balloc.take_released_memory()
        };
        #[cfg(not(feature = "level-1"))]
        if let Some((start, size)) = released {
            debug!("release empty heap pool: [{start:#x}, {:#x})", start + size);
            self.palloc.lock().dealloc_pages(start, size / PAGE_SIZE);
        }
        #[cfg(feature = "level-1")]
        let _ = released;
    }

    /// Allocates contiguous pages.
    ///
    /// It allocates `num_pages` pages from the page allocator.
    ///
    /// `align_pow2` must be a power of 2, and the returned region bound will be
    /// aligned to it.
    pub fn alloc_pages(
        &self,
        num_pages: usize,
        align_pow2: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        let addr = match self.allocate_pages_raw(num_pages, align_pow2) {
            Ok(addr) => addr,
            Err(first_error) => {
                self.drain_page_caches();
                self.allocate_pages_raw(num_pages, align_pow2)
                    .map_err(|_| first_error)?
            }
        };

        if cfg!(feature = "level-1") || !matches!(kind, UsageKind::RustHeap) {
            self.usages.alloc(kind, num_pages * PAGE_SIZE);
        }
        Ok(addr)
    }

    fn allocate_pages_raw(&self, num_pages: usize, align_pow2: usize) -> AllocResult<usize> {
        #[cfg(feature = "level-1")]
        {
            // single-level allocator: allocate from the byte allocator.
            let mut balloc = self.balloc.lock();
            let layout = Layout::from_size_align(num_pages * PAGE_SIZE, align_pow2).unwrap();
            let ptr = balloc.alloc(layout)?;
            Ok(ptr.as_ptr() as usize)
        }
        #[cfg(not(feature = "level-1"))]
        {
            self.palloc.lock().alloc_pages(num_pages, align_pow2)
        }
    }

    /// Allocates one order-0 page through the per-CPU page cache.
    pub fn alloc_page_cached(
        &self,
        cpu_id: usize,
        cpu_count: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        let total_pages = self.total_pages();
        let slot = cpu_id % PCP_CPU_SLOTS;

        let page = {
            let mut cache = self.page_caches[slot].lock();
            let page = cache.try_allocate(total_pages, cpu_count, kind);
            self.page_cache_counts[slot]
                .0
                .store(cache.count(), Ordering::Relaxed);
            page
        };
        if let Some(page) = page {
            return Ok(page);
        }

        self.alloc_page_cached_slow(slot, total_pages, cpu_count, kind)
    }

    #[cold]
    #[inline(never)]
    fn alloc_page_cached_slow(
        &self,
        slot: usize,
        total_pages: usize,
        cpu_count: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        if let Some(page) = self.refill_page_cache(slot, total_pages, cpu_count, kind) {
            return Ok(page);
        }

        self.drain_page_caches();
        match self.refill_page_cache(slot, total_pages, cpu_count, kind) {
            Some(page) => Ok(page),
            None => self.alloc_pages(1, PAGE_SIZE, kind),
        }
    }

    fn refill_page_cache(
        &self,
        slot: usize,
        total_pages: usize,
        cpu_count: usize,
        kind: UsageKind,
    ) -> Option<usize> {
        // Refill may acquire the global page allocator while the PCP slot is
        // locked. Keep the task on this CPU across that nested lock; the
        // common cache-hit path only needs the slot's IRQ guard.
        let _preempt_guard = NoPreemptIrqSave::new();
        let mut cache = self.page_caches[slot].lock();
        let page = cache.allocate(self, total_pages, cpu_count, kind);
        self.page_cache_counts[slot]
            .0
            .store(cache.count(), Ordering::Relaxed);
        page
    }

    /// Returns one order-0 page to the per-CPU page cache.
    pub fn dealloc_page_cached(
        &self,
        cpu_id: usize,
        cpu_count: usize,
        page: usize,
        kind: UsageKind,
    ) {
        assert_ne!(page, 0, "cached page address is null");
        assert_eq!(page % PAGE_SIZE, 0, "cached page is not page-aligned");

        let total_pages = self.total_pages();
        let slot = cpu_id % PCP_CPU_SLOTS;
        let _preempt_guard = NoPreemptIrqSave::new();
        let mut cache = self.page_caches[slot].lock();
        cache.release(self, total_pages, cpu_count, page, kind);
        self.page_cache_counts[slot]
            .0
            .store(cache.count(), Ordering::Relaxed);
    }

    /// Drains all per-CPU order-0 page caches into the global page allocator.
    pub fn drain_page_caches(&self) -> usize {
        let _preempt_guard = NoPreemptIrqSave::new();
        let mut drained = 0;
        for (slot, cache) in self.page_caches.iter().enumerate() {
            let mut cache = cache.lock();
            drained += cache.drain_all(self);
            self.page_cache_counts[slot]
                .0
                .store(cache.count(), Ordering::Relaxed);
        }
        drained
    }

    fn cached_page_count(&self) -> usize {
        self.page_cache_counts
            .iter()
            .map(|count| count.0.load(Ordering::Relaxed))
            .sum()
    }

    fn allocate_page_list(&self, pages: usize, kind: UsageKind) -> (usize, usize) {
        let (head, allocated) = {
            #[cfg(feature = "level-1")]
            {
                let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
                let mut balloc = self.balloc.lock();
                let mut head = 0;
                let mut allocated = 0;
                for _ in 0..pages {
                    let Ok(ptr) = balloc.alloc(layout) else {
                        break;
                    };
                    let page = ptr.as_ptr() as usize;
                    unsafe { (page as *mut usize).write(head) };
                    head = page;
                    allocated += 1;
                }
                (head, allocated)
            }
            #[cfg(not(feature = "level-1"))]
            {
                let mut palloc = self.palloc.lock();
                let mut head = 0;
                let mut allocated = 0;
                for _ in 0..pages {
                    let Ok(addr) = palloc.alloc_pages(1, PAGE_SIZE) else {
                        break;
                    };
                    unsafe { (addr as *mut usize).write(head) };
                    head = addr;
                    allocated += 1;
                }
                (head, allocated)
            }
        };

        if allocated != 0 && !matches!(kind, UsageKind::RustHeap) {
            self.usages.alloc(kind, allocated * PAGE_SIZE);
        }
        (head, allocated)
    }

    fn deallocate_page_lists(&self, heads: &[usize], counts: &[usize]) {
        assert_eq!(heads.len(), UsageKind::VARIANTS.len());
        assert_eq!(heads.len(), counts.len());
        let total: usize = counts.iter().sum();
        if total == 0 {
            return;
        }

        #[cfg(feature = "level-1")]
        {
            let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
            let mut balloc = self.balloc.lock();
            for (&head, &count) in heads.iter().zip(counts) {
                let mut page = head;
                for _ in 0..count {
                    debug_assert_ne!(page, 0);
                    let next = unsafe { (page as *const usize).read() };
                    let ptr = NonNull::new(page as *mut u8).unwrap();
                    balloc.dealloc(ptr, layout);
                    page = next;
                }
                debug_assert_eq!(page, 0);
            }
        }
        #[cfg(not(feature = "level-1"))]
        {
            let mut palloc = self.palloc.lock();
            for (&head, &count) in heads.iter().zip(counts) {
                let mut page = head;
                for _ in 0..count {
                    debug_assert_ne!(page, 0);
                    let next = unsafe { (page as *const usize).read() };
                    palloc.dealloc_pages(page, 1);
                    page = next;
                }
                debug_assert_eq!(page, 0);
            }
        }

        for &kind in UsageKind::VARIANTS {
            let pages = counts[kind as usize];
            if pages != 0 && !matches!(kind, UsageKind::RustHeap) {
                self.usages.dealloc(kind, pages * PAGE_SIZE);
            }
        }
    }

    /// Allocates contiguous pages starting from the given address.
    ///
    /// It allocates `num_pages` pages from the page allocator starting from the
    /// given address.
    ///
    /// `align_pow2` must be a power of 2, and the returned region bound will be
    /// aligned to it.
    pub fn alloc_pages_at(
        &self,
        start: usize,
        num_pages: usize,
        align_pow2: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        #[cfg(feature = "level-1")]
        {
            let _ = (start, num_pages, align_pow2, kind);
            unimplemented!("level-1 allocator does not support alloc_pages_at")
        }
        #[cfg(not(feature = "level-1"))]
        {
            let addr = match self.allocate_pages_at_raw(start, num_pages, align_pow2) {
                Ok(addr) => addr,
                Err(first_error) => {
                    self.drain_page_caches();
                    self.allocate_pages_at_raw(start, num_pages, align_pow2)
                        .map_err(|_| first_error)?
                }
            };
            if !matches!(kind, UsageKind::RustHeap) {
                self.usages.alloc(kind, num_pages * PAGE_SIZE);
            }
            Ok(addr)
        }
    }

    #[cfg(not(feature = "level-1"))]
    fn allocate_pages_at_raw(
        &self,
        start: usize,
        num_pages: usize,
        align_pow2: usize,
    ) -> AllocResult<usize> {
        self.palloc
            .lock()
            .alloc_pages_at(start, num_pages, align_pow2)
    }

    /// Gives back the allocated pages starts from `pos` to the page allocator.
    ///
    /// The pages should be allocated by [`alloc_pages`], and `align_pow2`
    /// should be the same as the one used in [`alloc_pages`]. Otherwise, the
    /// behavior is undefined.
    ///
    /// [`alloc_pages`]: GlobalAllocator::alloc_pages
    pub fn dealloc_pages(&self, pos: usize, num_pages: usize, kind: UsageKind) {
        #[cfg(feature = "level-1")]
        {
            // single-level allocator: deallocate to the byte allocator.
            let mut balloc = self.balloc.lock();
            let layout = Layout::from_size_align(num_pages * PAGE_SIZE, PAGE_SIZE).unwrap();
            let ptr = NonNull::new(pos as *mut u8).unwrap();
            balloc.dealloc(ptr, layout);
        }
        #[cfg(not(feature = "level-1"))]
        self.palloc.lock().dealloc_pages(pos, num_pages);
        self.usages.dealloc(kind, num_pages * PAGE_SIZE);
    }

    /// Returns the number of allocated bytes in the byte allocator.
    pub fn used_bytes(&self) -> usize {
        self.balloc.lock().used_bytes()
    }

    /// Returns the number of available bytes in the byte allocator.
    pub fn available_bytes(&self) -> usize {
        self.balloc.lock().available_bytes()
    }

    /// Returns the number of allocated pages in the page allocator.
    pub fn used_pages(&self) -> usize {
        let cached = self.cached_page_count();
        #[cfg(feature = "level-1")]
        {
            self.used_bytes().div_ceil(PAGE_SIZE).saturating_sub(cached)
        }
        #[cfg(not(feature = "level-1"))]
        {
            self.palloc.lock().used_pages().saturating_sub(cached)
        }
    }

    /// Returns the number of available pages in the page allocator.
    pub fn available_pages(&self) -> usize {
        let cached = self.cached_page_count();
        #[cfg(feature = "level-1")]
        {
            self.available_bytes().div_ceil(PAGE_SIZE) + cached
        }
        #[cfg(not(feature = "level-1"))]
        {
            self.palloc.lock().available_pages() + cached
        }
    }

    /// Returns the number of pages managed by the allocator.
    pub fn total_pages(&self) -> usize {
        self.managed_pages.load(Ordering::Relaxed)
    }

    /// Returns the usage statistics of the allocator.
    pub fn usages(&self) -> Usages {
        self.usages.snapshot()
    }
}

unsafe impl GlobalAlloc for GlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let inner = move || {
            if let Ok(ptr) = GlobalAllocator::alloc(self, layout) {
                ptr.as_ptr()
            } else {
                alloc::alloc::handle_alloc_error(layout)
            }
        };

        #[cfg(feature = "tracking")]
        {
            tracking::with_state(|state| match state {
                None => inner(),
                Some(state) => {
                    let ptr = inner();
                    let generation = state.generation;
                    state.generation += 1;
                    state.map.insert(
                        ptr as usize,
                        tracking::AllocationInfo {
                            layout,
                            backtrace: axbacktrace::Backtrace::capture(),
                            generation,
                        },
                    );
                    ptr
                }
            })
        }

        #[cfg(not(feature = "tracking"))]
        inner()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let ptr = NonNull::new(ptr).expect("dealloc null ptr");
        let inner = || GlobalAllocator::dealloc(self, ptr, layout);

        #[cfg(feature = "tracking")]
        tracking::with_state(|state| match state {
            None => inner(),
            Some(state) => {
                let address = ptr.as_ptr() as usize;
                state.map.remove(&address);
                inner()
            }
        });

        #[cfg(not(feature = "tracking"))]
        inner();
    }
}

#[cfg_attr(all(target_os = "none", not(test)), global_allocator)]
static GLOBAL_ALLOCATOR: GlobalAllocator = GlobalAllocator::new();

/// Returns the reference to the global allocator.
pub fn global_allocator() -> &'static GlobalAllocator {
    &GLOBAL_ALLOCATOR
}

/// Initializes the global allocator with the given memory region.
///
/// Note that the memory region bounds are just numbers, and the allocator
/// does not actually access the region. Users should ensure that the region
/// is valid and not being used by others, so that the allocated memory is also
/// valid.
///
/// This function should be called only once, and before any allocation.
pub fn global_init(start_vaddr: usize, size: usize) {
    debug!(
        "initialize global allocator at: [{:#x}, {:#x})",
        start_vaddr,
        start_vaddr + size
    );
    GLOBAL_ALLOCATOR.init(start_vaddr, size);
}

/// Add the given memory region to the global allocator.
///
/// Users should ensure that the region is valid and not being used by others,
/// so that the allocated memory is also valid.
///
/// It's similar to [`global_init`], but can be called multiple times.
pub fn global_add_memory(start_vaddr: usize, size: usize) -> AllocResult {
    debug!(
        "add a memory region to global allocator: [{:#x}, {:#x})",
        start_vaddr,
        start_vaddr + size
    );
    GLOBAL_ALLOCATOR.add_memory(start_vaddr, size)
}

#[cfg(all(test, feature = "slab", not(feature = "level-1")))]
mod tests {
    extern crate std;

    use std::alloc::{alloc_zeroed, dealloc};

    use super::*;

    const LARGE_BLOCK_SIZE: usize = 16 * 1024 * 1024;

    #[test]
    fn slab_heap_growth_aligns_large_buddy_pool() {
        const BACKING_SIZE: usize = 3 * LARGE_BLOCK_SIZE;
        const MANAGED_SIZE: usize = 2 * LARGE_BLOCK_SIZE;

        let backing_layout = Layout::from_size_align(BACKING_SIZE, LARGE_BLOCK_SIZE).unwrap();
        let backing = unsafe { alloc_zeroed(backing_layout) };
        assert!(!backing.is_null());

        // Deliberately begin the managed range one page after a 16-MiB
        // boundary.  A page-aligned 16-MiB pool cannot serve a 16-MiB buddy
        // allocation, while a size-aligned pool selected from this same range
        // can.
        {
            let allocator = GlobalAllocator::new();
            allocator.init(backing as usize + PAGE_SIZE, MANAGED_SIZE);

            let layout = Layout::from_size_align(LARGE_BLOCK_SIZE - 0x100, 1).unwrap();
            let ptr = allocator
                .alloc(layout)
                .expect("large allocation should use an aligned heap pool");
            assert_eq!(ptr.as_ptr() as usize % LARGE_BLOCK_SIZE, 0);
            allocator.dealloc(ptr, layout);
        }

        unsafe { dealloc(backing, backing_layout) };
    }
}
