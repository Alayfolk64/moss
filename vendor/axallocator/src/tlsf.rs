//! The TLSF (Two-Level Segregated Fit) dynamic memory allocation algorithm.
//!
//! This module wraps the implementation provided by the [rlsf] crate.

use core::{alloc::Layout, ptr::NonNull};

use rlsf::Tlsf;

use super::{AllocError, AllocResult, BaseAllocator, ByteAllocator};

const MAX_POOLS: usize = 64;

struct TlsfPool {
    inner: Tlsf<'static, u32, u32, 28, 32>,
    start: usize,
    size: usize,
    used_bytes: usize,
    active: bool,
    reclaimable: bool,
}

impl TlsfPool {
    const fn empty() -> Self {
        Self {
            inner: Tlsf::new(),
            start: 0,
            size: 0,
            used_bytes: 0,
            active: false,
            reclaimable: false,
        }
    }

    fn contains(&self, ptr: usize) -> bool {
        self.active && ptr >= self.start && ptr < self.start.saturating_add(self.size)
    }

    fn reset(&mut self) {
        // `used_bytes == 0` guarantees that the TLSF contains no live
        // allocation. Replacing its list heads removes every remaining
        // reference to the backing region before the caller returns its pages.
        *self = Self::empty();
    }
}

/// A TLSF (Two-Level Segregated Fit) memory allocator.
///
/// It's just a wrapper structure of [`rlsf::Tlsf`], with `FLLEN` and `SLLEN`
/// fixed to 28 and 32.
pub struct TlsfByteAllocator {
    pools: [TlsfPool; MAX_POOLS],
    current_pool: usize,
    total_bytes: usize,
    used_bytes: usize,
    released_memory: Option<(usize, usize)>,
}

impl TlsfByteAllocator {
    /// Creates a new empty [`TlsfByteAllocator`].
    pub const fn new() -> Self {
        Self {
            pools: [const { TlsfPool::empty() }; MAX_POOLS],
            current_pool: 0,
            total_bytes: 0,
            used_bytes: 0,
            released_memory: None,
        }
    }

    fn add_pool(&mut self, start: usize, size: usize, reclaimable: bool) -> AllocResult {
        let slot = self
            .pools
            .iter()
            .position(|pool| !pool.active)
            .ok_or(AllocError::NoMemory)?;
        let actual_size = unsafe {
            let region = core::slice::from_raw_parts_mut(start as *mut u8, size);
            self.pools[slot]
                .inner
                .insert_free_block_ptr(NonNull::new(region).ok_or(AllocError::InvalidParam)?)
                .ok_or(AllocError::InvalidParam)?
                .get()
        };
        self.pools[slot].start = start;
        self.pools[slot].size = actual_size;
        self.pools[slot].active = true;
        self.pools[slot].reclaimable = reclaimable;
        self.current_pool = slot;
        self.total_bytes += actual_size;
        Ok(())
    }
}

impl Default for TlsfByteAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl BaseAllocator for TlsfByteAllocator {
    fn init(&mut self, start: usize, size: usize) {
        *self = Self::new();
        self.add_pool(start, size, false)
            .expect("invalid initial TLSF memory pool");
    }

    fn add_memory(&mut self, start: usize, size: usize) -> AllocResult {
        self.add_pool(start, size, true)
    }
}

impl ByteAllocator for TlsfByteAllocator {
    fn alloc(&mut self, layout: Layout) -> AllocResult<NonNull<u8>> {
        let current = self.current_pool;
        if self.pools[current].active {
            if let Some(ptr) = self.pools[current].inner.allocate(layout) {
                self.pools[current].used_bytes += layout.size();
                self.used_bytes += layout.size();
                return Ok(ptr);
            }
        }

        for (index, pool) in self.pools.iter_mut().enumerate() {
            if index == current || !pool.active {
                continue;
            }
            if let Some(ptr) = pool.inner.allocate(layout) {
                pool.used_bytes += layout.size();
                self.used_bytes += layout.size();
                self.current_pool = index;
                return Ok(ptr);
            }
        }
        Err(AllocError::NoMemory)
    }

    fn dealloc(&mut self, pos: NonNull<u8>, layout: Layout) {
        let index = self
            .pools
            .iter()
            .position(|pool| pool.contains(pos.as_ptr() as usize))
            .expect("deallocating pointer outside TLSF pools");
        let pool = &mut self.pools[index];
        unsafe { pool.inner.deallocate(pos, layout.align()) }
        pool.used_bytes = pool
            .used_bytes
            .checked_sub(layout.size())
            .expect("TLSF pool usage underflow");
        self.used_bytes -= layout.size();

        if pool.reclaimable && pool.used_bytes == 0 {
            assert!(
                self.released_memory.is_none(),
                "released TLSF pool was not consumed"
            );
            let released = (pool.start, pool.size);
            self.total_bytes -= pool.size;
            pool.reset();
            self.released_memory = Some(released);
            if self.current_pool == index {
                self.current_pool = self
                    .pools
                    .iter()
                    .rposition(|pool| pool.active)
                    .expect("initial TLSF pool disappeared");
            }
        }
    }

    fn take_released_memory(&mut self) -> Option<(usize, usize)> {
        self.released_memory.take()
    }

    fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    fn available_bytes(&self) -> usize {
        self.total_bytes - self.used_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(align(4096))]
    struct AlignedPool([u8; 64 * 1024]);

    #[test]
    fn releases_only_an_empty_added_pool() {
        let mut initial = AlignedPool([0; 64 * 1024]);
        let mut added = AlignedPool([0; 64 * 1024]);
        let mut allocator = TlsfByteAllocator::new();
        allocator.init(initial.0.as_mut_ptr() as usize, initial.0.len());
        allocator
            .add_memory(added.0.as_mut_ptr() as usize, added.0.len())
            .unwrap();

        let layout = Layout::from_size_align(4096, 64).unwrap();
        let ptr = allocator.alloc(layout).unwrap();
        let added_range = added.0.as_ptr() as usize..added.0.as_ptr() as usize + added.0.len();
        assert!(added_range.contains(&(ptr.as_ptr() as usize)));
        assert_eq!(allocator.take_released_memory(), None);

        allocator.dealloc(ptr, layout);
        assert_eq!(
            allocator.take_released_memory(),
            Some((added.0.as_mut_ptr() as usize, added.0.len()))
        );
        assert_eq!(allocator.total_bytes(), initial.0.len());

        let initial_ptr = allocator.alloc(layout).unwrap();
        allocator.dealloc(initial_ptr, layout);
        assert_eq!(allocator.take_released_memory(), None);
    }
}
