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
use kspin::SpinNoIrq;
use strum::{IntoStaticStr, VariantArray};

const PAGE_SIZE: usize = 0x1000;
const MIN_HEAP_SIZE: usize = 0x8000; // 32 K

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
    const fn new() -> Self {
        Self([0; UsageKind::VARIANTS.len()])
    }

    fn alloc(&mut self, kind: UsageKind, size: usize) {
        self.0[kind as usize] += size;
    }

    fn dealloc(&mut self, kind: UsageKind, size: usize) {
        self.0[kind as usize] -= size;
    }

    pub fn get(&self, kind: UsageKind) -> usize {
        self.0[kind as usize]
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
    page_caches: [SpinNoIrq<PerCpuPageCache>; PCP_CPU_SLOTS],
    page_cache_counts: [CacheAlignedCount; PCP_CPU_SLOTS],
    managed_pages: AtomicUsize,
    usages: SpinNoIrq<Usages>,
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
            page_caches: [const { SpinNoIrq::new(PerCpuPageCache::new()) }; PCP_CPU_SLOTS],
            page_cache_counts: [const { CacheAlignedCount::new() }; PCP_CPU_SLOTS],
            managed_pages: AtomicUsize::new(0),
            usages: SpinNoIrq::new(Usages::new()),
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
    /// It will add the whole region to the byte allocator.
    pub fn add_memory(&self, start_vaddr: usize, size: usize) -> AllocResult {
        let result = self.balloc.lock().add_memory(start_vaddr, size);
        #[cfg(feature = "level-1")]
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
        self.usages.lock().alloc(UsageKind::RustHeap, layout.size());
        Ok(ptr)
    }

    #[cfg(not(feature = "level-1"))]
    fn alloc_level2(&self, layout: Layout) -> AllocResult<NonNull<u8>> {
        // simple two-level allocator: if no heap memory, allocate from the page allocator.
        let mut balloc = self.balloc.lock();
        loop {
            if let Ok(ptr) = balloc.alloc(layout) {
                self.usages.lock().alloc(UsageKind::RustHeap, layout.size());
                return Ok(ptr);
            } else {
                let old_size = balloc.total_bytes();
                let expand_size = old_size
                    .max(layout.size())
                    .next_power_of_two()
                    .max(PAGE_SIZE);

                let mut try_size = expand_size;
                let min_size = PAGE_SIZE.max(layout.size());
                loop {
                    let heap_ptr = match self.alloc_pages(
                        try_size / PAGE_SIZE,
                        PAGE_SIZE,
                        UsageKind::RustHeap,
                    ) {
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
        self.usages
            .lock()
            .dealloc(UsageKind::RustHeap, layout.size());
        self.balloc.lock().dealloc(pos, layout)
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
            self.usages.lock().alloc(kind, num_pages * PAGE_SIZE);
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

        for attempt in 0..2 {
            let page = {
                let mut cache = self.page_caches[slot].lock();
                let page = cache.allocate(self, total_pages, cpu_count, kind);
                self.page_cache_counts[slot]
                    .0
                    .store(cache.count(), Ordering::Relaxed);
                page
            };
            if let Some(page) = page {
                return Ok(page);
            }
            if attempt == 0 {
                self.drain_page_caches();
            }
        }

        self.alloc_pages(1, PAGE_SIZE, kind)
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
        let mut cache = self.page_caches[slot].lock();
        cache.release(self, total_pages, cpu_count, page, kind);
        self.page_cache_counts[slot]
            .0
            .store(cache.count(), Ordering::Relaxed);
    }

    /// Drains all per-CPU order-0 page caches into the global page allocator.
    ///
    /// This is called automatically before retrying a failed global page
    /// allocation, so cached pages cannot cause an avoidable OOM or prevent a
    /// high-order contiguous allocation.
    pub fn drain_page_caches(&self) -> usize {
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
            self.usages.lock().alloc(kind, allocated * PAGE_SIZE);
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

        let mut usages = self.usages.lock();
        for &kind in UsageKind::VARIANTS {
            let pages = counts[kind as usize];
            if pages != 0 && !matches!(kind, UsageKind::RustHeap) {
                usages.dealloc(kind, pages * PAGE_SIZE);
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
                self.usages.lock().alloc(kind, num_pages * PAGE_SIZE);
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
        self.usages.lock().dealloc(kind, num_pages * PAGE_SIZE);
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
        *self.usages.lock()
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
