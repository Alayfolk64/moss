use core::{
    marker::PhantomData,
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

use arrayvec::ArrayVec;
use memory_addr::{MemoryAddr, PAGE_SIZE_4K, PhysAddr};

use crate::{
    GenericPTE, MappingFlags, PageSize, PagingError, PagingHandler, PagingMetaData, PagingResult,
};

const ENTRY_COUNT: usize = 512;

const fn p4_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 27)) & (ENTRY_COUNT - 1)
}

const fn p3_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 18)) & (ENTRY_COUNT - 1)
}

const fn p2_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 9)) & (ENTRY_COUNT - 1)
}

const fn p1_index(vaddr: usize) -> usize {
    (vaddr >> 12) & (ENTRY_COUNT - 1)
}

/// A generic page table struct for 64-bit platform.
///
/// It also tracks all intermediate level tables. They will be deallocated
/// When the [`PageTable64`] itself is dropped.
pub struct PageTable64<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> {
    root_paddr: PhysAddr,
    #[cfg(feature = "copy-from")]
    borrowed_entries: bitmaps::Bitmap<ENTRY_COUNT>,
    _phantom: PhantomData<(M, PTE, H)>,
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> PageTable64<M, PTE, H> {
    /// Creates a new page table instance or returns the error.
    ///
    /// It will allocate a new page for the root page table.
    pub fn try_new() -> PagingResult<Self> {
        let root_paddr = Self::alloc_table()?;
        Ok(Self {
            root_paddr,
            #[cfg(feature = "copy-from")]
            borrowed_entries: bitmaps::Bitmap::new(),
            _phantom: PhantomData,
        })
    }

    /// Returns the physical address of the root page table.
    pub const fn root_paddr(&self) -> PhysAddr {
        self.root_paddr
    }

    /// Queries the result of the mapping starts with `vaddr`.
    ///
    /// Returns the physical address of the target frame, mapping flags, and
    /// the page size.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn query(&self, vaddr: M::VirtAddr) -> PagingResult<(PhysAddr, MappingFlags, PageSize)> {
        let (entry, size) = self.get_entry(vaddr)?;
        if !entry.is_present() {
            return Err(PagingError::NotMapped);
        }
        let off = size.align_offset(vaddr.into());
        Ok((entry.paddr().add(off), entry.flags(), size))
    }

    /// Finds the first present mapping that overlaps `[start, end)`.
    ///
    /// Missing entries are skipped at their page-table level, so sparse ranges
    /// are traversed by populated page-table nodes instead of 4-KiB pages.
    pub fn next_present_mapping(
        &self,
        start: M::VirtAddr,
        end: M::VirtAddr,
    ) -> Option<(M::VirtAddr, PhysAddr, MappingFlags, PageSize)> {
        let start: usize = start.into();
        let end: usize = end.into();
        if start >= end {
            return None;
        }
        self.next_present_in_table(self.root_paddr, 0, start, end)
    }

    /// Gets a mutable reference to modify the page table.
    ///
    /// The TLB will be flushed automatically when the mutable reference is
    /// dropped.
    pub fn modify(&mut self) -> PageTable64Mut<'_, M, PTE, H> {
        PageTable64Mut::new(self)
    }
}

// Private implements.
impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> PageTable64<M, PTE, H> {
    fn alloc_table() -> PagingResult<PhysAddr> {
        if let Some(paddr) = H::alloc_frame() {
            let ptr = H::phys_to_virt(paddr).as_mut_ptr();
            unsafe { core::ptr::write_bytes(ptr, 0, PAGE_SIZE_4K) };
            Ok(paddr)
        } else {
            Err(PagingError::NoMemory)
        }
    }

    fn table_of<'a>(&self, paddr: PhysAddr) -> &'a [PTE] {
        let ptr = H::phys_to_virt(paddr).as_ptr() as _;
        unsafe { core::slice::from_raw_parts(ptr, ENTRY_COUNT) }
    }

    fn atomic_entry(&self, table: PhysAddr, index: usize) -> &AtomicUsize {
        debug_assert!(index < ENTRY_COUNT);
        let ptr = H::phys_to_virt(table).as_ptr() as *const AtomicUsize;
        unsafe { &*ptr.add(index) }
    }

    fn load_entry(&self, table: PhysAddr, index: usize) -> PTE {
        PTE::from_bits(self.atomic_entry(table, index).load(Ordering::Acquire))
    }

    fn next_present_in_table(
        &self,
        table: PhysAddr,
        level: usize,
        start: usize,
        end: usize,
    ) -> Option<(M::VirtAddr, PhysAddr, MappingFlags, PageSize)> {
        debug_assert!(level < M::LEVELS);
        let shift = 12 + 9 * (M::LEVELS - level - 1);
        let address_bits = usize::BITS as usize;
        let span = (shift < address_bits).then(|| 1usize << shift);
        let mut addr = start;

        while addr < end {
            let index = if shift < address_bits {
                (addr >> shift) & (ENTRY_COUNT - 1)
            } else {
                0
            };
            let mapping_base = span.map_or(0, |span| addr & !(span - 1));
            let entry_end = span.map_or(end, |span| {
                mapping_base
                    .checked_add(span)
                    .unwrap_or(usize::MAX)
                    .min(end)
            });
            let entry = self.load_entry(table, index);

            if !entry.is_unused() {
                if level + 1 == M::LEVELS || entry.is_huge() {
                    if entry.is_present() {
                        let span = span?;
                        let page_size = match span {
                            PAGE_SIZE_4K => PageSize::Size4K,
                            0x20_0000 => PageSize::Size2M,
                            0x4000_0000 => PageSize::Size1G,
                            _ => return None,
                        };
                        return Some((
                            mapping_base.into(),
                            entry.paddr(),
                            entry.flags(),
                            page_size,
                        ));
                    }
                } else if let Ok(next_table) = self.next_table_paddr(entry)
                    && let Some(found) =
                        self.next_present_in_table(next_table, level + 1, addr, entry_end)
                {
                    return Some(found);
                }
            }

            if entry_end <= addr {
                break;
            }
            addr = entry_end;
        }
        None
    }

    fn next_table<'a>(&self, entry: &PTE) -> PagingResult<&'a [PTE]> {
        if entry.paddr().as_usize() == 0 {
            Err(PagingError::NotMapped)
        } else if entry.is_huge() {
            Err(PagingError::MappedToHugePage)
        } else {
            Ok(self.table_of(entry.paddr()))
        }
    }

    fn next_table_paddr(&self, entry: PTE) -> PagingResult<PhysAddr> {
        if entry.paddr().as_usize() == 0 {
            Err(PagingError::NotMapped)
        } else if entry.is_huge() {
            Err(PagingError::MappedToHugePage)
        } else {
            Ok(entry.paddr())
        }
    }

    fn get_entry(&self, vaddr: M::VirtAddr) -> PagingResult<(PTE, PageSize)> {
        let vaddr: usize = vaddr.into();
        let p3 = if M::LEVELS == 3 {
            self.root_paddr()
        } else if M::LEVELS == 4 {
            let p4e = self.load_entry(self.root_paddr(), p4_index(vaddr));
            self.next_table_paddr(p4e)?
        } else {
            unreachable!()
        };
        let p3e = self.load_entry(p3, p3_index(vaddr));
        if p3e.is_huge() {
            return Ok((p3e, PageSize::Size1G));
        }

        let p2 = self.next_table_paddr(p3e)?;
        let p2e = self.load_entry(p2, p2_index(vaddr));
        if p2e.is_huge() {
            return Ok((p2e, PageSize::Size2M));
        }

        let p1 = self.next_table_paddr(p2e)?;
        let p1e = self.load_entry(p1, p1_index(vaddr));
        Ok((p1e, PageSize::Size4K))
    }

    fn next_table_atomic_or_create(&self, table: PhysAddr, index: usize) -> PagingResult<PhysAddr> {
        let slot = self.atomic_entry(table, index);
        loop {
            let current_bits = slot.load(Ordering::Acquire);
            let current = PTE::from_bits(current_bits);
            if !current.is_unused() {
                return self.next_table_paddr(current);
            }

            let new_table = Self::alloc_table()?;
            let new_bits = PTE::new_table(new_table).bits();
            match slot.compare_exchange(current_bits, new_bits, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(new_table),
                Err(_) => H::dealloc_frame(new_table),
            }
        }
    }

    fn leaf_entry_atomic(&self, vaddr: M::VirtAddr, create: bool) -> PagingResult<&AtomicUsize> {
        let addr: usize = vaddr.into();
        let next = |table, index| {
            if create {
                self.next_table_atomic_or_create(table, index)
            } else {
                self.next_table_paddr(self.load_entry(table, index))
            }
        };

        let p3 = if M::LEVELS == 3 {
            self.root_paddr()
        } else if M::LEVELS == 4 {
            next(self.root_paddr(), p4_index(addr))?
        } else {
            unreachable!()
        };
        let p2 = next(p3, p3_index(addr))?;
        let p1 = next(p2, p2_index(addr))?;
        Ok(self.atomic_entry(p1, p1_index(addr)))
    }

    /// Installs one 4-KiB mapping using compare-and-exchange.
    pub fn map_4k_concurrent(
        &self,
        vaddr: M::VirtAddr,
        target: PhysAddr,
        flags: MappingFlags,
    ) -> PagingResult {
        let slot = self.leaf_entry_atomic(vaddr, true)?;
        let entry = PTE::new_page(target.align_down(PageSize::Size4K), flags, false).bits();
        slot.compare_exchange(0, entry, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| PagingError::AlreadyMapped)
    }

    /// Replaces one 4-KiB mapping only if it still points to the expected
    /// physical page.
    pub fn remap_4k_concurrent_if(
        &self,
        vaddr: M::VirtAddr,
        expected_paddr: PhysAddr,
        target: PhysAddr,
        flags: MappingFlags,
    ) -> PagingResult<bool> {
        let slot = self.leaf_entry_atomic(vaddr, false)?;
        loop {
            let old_bits = slot.load(Ordering::Acquire);
            let old = PTE::from_bits(old_bits);
            if !old.is_present() {
                return Err(PagingError::NotMapped);
            }
            if old.paddr() != expected_paddr.align_down(PageSize::Size4K) {
                return Ok(false);
            }
            let new_bits = PTE::new_page(target.align_down(PageSize::Size4K), flags, false).bits();
            match slot.compare_exchange(old_bits, new_bits, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    M::flush_tlb(Some(vaddr));
                    return Ok(true);
                }
                Err(_) => continue,
            }
        }
    }

    fn dealloc_tree(&self, table_paddr: PhysAddr, level: usize) {
        // don't free the entries in last level, they are not array.
        if level < M::LEVELS - 1 {
            for entry in self.table_of(table_paddr) {
                if self.next_table(entry).is_ok() {
                    self.dealloc_tree(entry.paddr(), level + 1);
                }
            }
        }
        H::dealloc_frame(table_paddr);
    }
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> Drop for PageTable64<M, PTE, H> {
    fn drop(&mut self) {
        let root = self.table_of(self.root_paddr);
        #[allow(unused_variables)]
        for (i, entry) in root.iter().enumerate() {
            #[cfg(feature = "copy-from")]
            if self.borrowed_entries.get(i) {
                continue;
            }
            if self.next_table(entry).is_ok() {
                self.dealloc_tree(entry.paddr(), 1);
            }
        }
        H::dealloc_frame(self.root_paddr());
    }
}

// TODO: tune threshold
const SMALL_FLUSH_THRESHOLD: usize = 16;

enum ToFlush<M: PagingMetaData> {
    None,
    Addresses(ArrayVec<M::VirtAddr, SMALL_FLUSH_THRESHOLD>),
    Full,
}

/// A mutable reference to a [`PageTable64`], allowing modifications and
/// flushing the TLB on drop.
pub struct PageTable64Mut<'a, M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> {
    inner: &'a mut PageTable64<M, PTE, H>,
    flush: ToFlush<M>,
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> Deref for PageTable64Mut<'_, M, PTE, H> {
    type Target = PageTable64<M, PTE, H>;

    fn deref(&self) -> &Self::Target {
        self.inner
    }
}

impl<'a, M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> PageTable64Mut<'a, M, PTE, H> {
    fn new(inner: &'a mut PageTable64<M, PTE, H>) -> Self {
        Self {
            inner,
            flush: ToFlush::None,
        }
    }

    fn flush(&mut self, vaddr: M::VirtAddr) {
        match self.flush {
            ToFlush::None => {
                let mut addresses = ArrayVec::new();
                addresses.push(vaddr);
                self.flush = ToFlush::Addresses(addresses);
            }
            ToFlush::Addresses(ref mut addrs) => {
                if addrs.try_push(vaddr).is_err() {
                    self.flush = ToFlush::Full;
                }
            }
            ToFlush::Full => {}
        }
    }

    fn table_of_mut(&mut self, paddr: PhysAddr) -> &'a mut [PTE] {
        let ptr = H::phys_to_virt(paddr).as_mut_ptr() as _;
        unsafe { core::slice::from_raw_parts_mut(ptr, ENTRY_COUNT) }
    }

    fn next_table_mut(&mut self, entry: &PTE) -> PagingResult<&'a mut [PTE]> {
        if entry.paddr().as_usize() == 0 {
            Err(PagingError::NotMapped)
        } else if entry.is_huge() {
            Err(PagingError::MappedToHugePage)
        } else {
            Ok(self.table_of_mut(entry.paddr()))
        }
    }

    fn next_table_mut_or_create(&mut self, entry: &mut PTE) -> PagingResult<&'a mut [PTE]> {
        if entry.is_unused() {
            let paddr = PageTable64::<M, PTE, H>::alloc_table()?;
            *entry = GenericPTE::new_table(paddr);
            Ok(self.table_of_mut(paddr))
        } else {
            self.next_table_mut(entry)
        }
    }

    fn get_entry_mut(&mut self, vaddr: M::VirtAddr) -> PagingResult<(&mut PTE, PageSize)> {
        let vaddr: usize = vaddr.into();
        let p3 = if M::LEVELS == 3 {
            self.table_of_mut(self.root_paddr())
        } else if M::LEVELS == 4 {
            let p4 = self.table_of_mut(self.root_paddr());
            let p4e = &mut p4[p4_index(vaddr)];
            self.next_table_mut(p4e)?
        } else {
            unreachable!()
        };
        let p3e = &mut p3[p3_index(vaddr)];
        if p3e.is_huge() {
            return Ok((p3e, PageSize::Size1G));
        }

        let p2 = self.next_table_mut(p3e)?;
        let p2e = &mut p2[p2_index(vaddr)];
        if p2e.is_huge() {
            return Ok((p2e, PageSize::Size2M));
        }

        let p1 = self.next_table_mut(p2e)?;
        let p1e = &mut p1[p1_index(vaddr)];
        Ok((p1e, PageSize::Size4K))
    }

    fn get_entry_mut_or_create(
        &mut self,
        vaddr: M::VirtAddr,
        page_size: PageSize,
    ) -> PagingResult<&mut PTE> {
        let vaddr: usize = vaddr.into();
        let p3 = if M::LEVELS == 3 {
            self.table_of_mut(self.root_paddr())
        } else if M::LEVELS == 4 {
            let p4 = self.table_of_mut(self.root_paddr());
            let p4e = &mut p4[p4_index(vaddr)];
            self.next_table_mut_or_create(p4e)?
        } else {
            unreachable!()
        };
        let p3e = &mut p3[p3_index(vaddr)];
        if page_size == PageSize::Size1G {
            return Ok(p3e);
        }

        let p2 = self.next_table_mut_or_create(p3e)?;
        let p2e = &mut p2[p2_index(vaddr)];
        if page_size == PageSize::Size2M {
            return Ok(p2e);
        }

        let p1 = self.next_table_mut_or_create(p2e)?;
        let p1e = &mut p1[p1_index(vaddr)];
        Ok(p1e)
    }

    /// Maps a virtual page to a physical frame with the given `page_size`
    /// and mapping `flags`.
    ///
    /// The virtual page starts with `vaddr`, and the physical frame starts with
    /// `target`. If the `target` is not aligned to the `page_size`, it will be
    /// aligned down automatically.
    ///
    /// Returns [`Err(PagingError::AlreadyMapped)`](PagingError::AlreadyMapped)
    /// if the mapping is already present.
    pub fn map(
        &mut self,
        vaddr: M::VirtAddr,
        target: PhysAddr,
        page_size: PageSize,
        flags: MappingFlags,
    ) -> PagingResult {
        // `vaddr` does not need to be page-aligned here; `get_entry_mut_or_create`
        // internally maps `vaddr` to its corresponding page table entry (PTE).
        let entry = self.get_entry_mut_or_create(vaddr, page_size)?;
        if !entry.is_unused() {
            return Err(PagingError::AlreadyMapped);
        }
        *entry = GenericPTE::new_page(target.align_down(page_size), flags, page_size.is_huge());
        // A newly installed mapping cannot replace a cached valid
        // translation on another CPU. Invalidate only the current CPU's
        // possible negative entry; no synchronous remote shootdown is needed.
        M::flush_tlb_local(Some(vaddr));
        Ok(())
    }

    /// Remap the mapping starts with `vaddr`, updates both the physical address
    /// and flags.
    ///
    /// Returns the page size of the mapping.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// intermediate level tables of the mapping is not present.
    pub fn remap(
        &mut self,
        vaddr: M::VirtAddr,
        paddr: PhysAddr,
        flags: MappingFlags,
    ) -> PagingResult<PageSize> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        entry.set_paddr(paddr);
        entry.set_flags(flags, size.is_huge());
        self.flush(vaddr);
        Ok(size)
    }

    /// Updates the flags of the mapping starts with `vaddr`.
    ///
    /// Returns the page size of the mapping.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn protect(&mut self, vaddr: M::VirtAddr, flags: MappingFlags) -> PagingResult<PageSize> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if !entry.is_present() {
            return Err(PagingError::NotMapped);
        }
        entry.set_flags(flags, size.is_huge());
        self.flush(vaddr);
        Ok(size)
    }

    /// Unmaps the mapping starts with `vaddr`.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn unmap(
        &mut self,
        vaddr: M::VirtAddr,
    ) -> PagingResult<(PhysAddr, MappingFlags, PageSize)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if !entry.is_present() {
            entry.clear();
            return Err(PagingError::NotMapped);
        }
        let paddr = entry.paddr();
        let flags = entry.flags();
        entry.clear();
        self.flush(vaddr);
        Ok((paddr, flags, size))
    }

    /// Maps a contiguous virtual memory region to a contiguous physical memory
    /// region with the given mapping `flags`.
    ///
    /// The virtual and physical memory regions start with `vaddr` and `paddr`
    /// respectively. The region size is `size`. The addresses and `size` must
    /// be aligned to 4K, otherwise it will return
    /// [`Err(PagingError::NotAligned)`].
    ///
    /// When `allow_huge` is true, it will try to map the region with huge pages
    /// if possible. Otherwise, it will map the region with 4K pages.
    ///
    /// [`Err(PagingError::NotAligned)`]: PagingError::NotAligned
    pub fn map_region(
        &mut self,
        vaddr: M::VirtAddr,
        get_paddr: impl Fn(M::VirtAddr) -> PhysAddr,
        size: usize,
        flags: MappingFlags,
        allow_huge: bool,
    ) -> PagingResult {
        let mut vaddr_usize: usize = vaddr.into();
        let mut size = size;
        if !PageSize::Size4K.is_aligned(vaddr_usize) || !PageSize::Size4K.is_aligned(size) {
            return Err(PagingError::NotAligned);
        }
        trace!(
            "map_region({:#x}): [{:#x}, {:#x}) {:?}",
            self.root_paddr(),
            vaddr_usize,
            vaddr_usize + size,
            flags,
        );
        while size > 0 {
            let vaddr = vaddr_usize.into();
            let paddr = get_paddr(vaddr);
            let page_size = if allow_huge {
                if PageSize::Size1G.is_aligned(vaddr_usize)
                    && paddr.is_aligned(PageSize::Size1G)
                    && size >= PageSize::Size1G as usize
                {
                    PageSize::Size1G
                } else if PageSize::Size2M.is_aligned(vaddr_usize)
                    && paddr.is_aligned(PageSize::Size2M)
                    && size >= PageSize::Size2M as usize
                {
                    PageSize::Size2M
                } else {
                    PageSize::Size4K
                }
            } else {
                PageSize::Size4K
            };
            self.map(vaddr, paddr, page_size, flags).inspect_err(|e| {
                error!("failed to map page: {vaddr_usize:#x?}({page_size:?}) -> {paddr:#x?}, {e:?}")
            })?;

            vaddr_usize += page_size as usize;
            size -= page_size as usize;
        }
        Ok(())
    }

    /// Unmaps a contiguous virtual memory region.
    ///
    /// The region must be mapped before using [`Self::map_region`], or
    /// unexpected behaviors may occur. It can deal with huge pages
    /// automatically.
    pub fn unmap_region(&mut self, vaddr: M::VirtAddr, size: usize) -> PagingResult {
        let mut vaddr_usize: usize = vaddr.into();
        let mut size = size;
        trace!(
            "unmap_region({:#x}) [{:#x}, {:#x})",
            self.root_paddr(),
            vaddr_usize,
            vaddr_usize + size,
        );
        while size > 0 {
            let vaddr = vaddr_usize.into();
            let (_, _, page_size) = self
                .unmap(vaddr)
                .inspect_err(|e| error!("failed to unmap page: {vaddr_usize:#x?}, {e:?}"))?;

            assert!(page_size.is_aligned(vaddr_usize));
            assert!(page_size as usize <= size);
            vaddr_usize += page_size as usize;
            size -= page_size as usize;
        }
        Ok(())
    }

    /// Updates mapping flags of a contiguous virtual memory region.
    ///
    /// The region must be mapped before using [`Self::map_region`], or
    /// unexpected behaviors may occur. It can deal with huge pages
    /// automatically.
    pub fn protect_region(
        &mut self,
        vaddr: M::VirtAddr,
        size: usize,
        flags: MappingFlags,
    ) -> PagingResult {
        let start: usize = vaddr.into();
        let Some(end) = start.checked_add(size) else {
            return Err(PagingError::NotAligned);
        };
        trace!(
            "protect_region({:#x}) [{:#x}, {:#x}) {:?}",
            self.root_paddr(),
            start,
            end,
            flags,
        );

        let mut cursor = start;
        while let Some((mapping, _paddr, _old_flags, page_size)) =
            self.next_present_mapping(cursor.into(), end.into())
        {
            let mapping: usize = mapping.into();
            self.protect(mapping.into(), flags).inspect_err(|e| {
                error!("failed to protect page: {mapping:#x?}, {e:?}");
            })?;
            let next = mapping
                .checked_add(page_size as usize)
                .ok_or(PagingError::NotAligned)?;
            if next <= cursor {
                return Err(PagingError::NotAligned);
            }
            cursor = next;
        }
        Ok(())
    }

    /// Copy entries from another page table within the given virtual memory
    /// range.
    #[cfg(feature = "copy-from")]
    pub fn copy_from(&mut self, other: &PageTable64<M, PTE, H>, start: M::VirtAddr, size: usize) {
        if size == 0 {
            return;
        }
        let src_table = self.table_of(other.root_paddr);
        let dst_table = self.table_of_mut(self.root_paddr);
        let index_fn = if M::LEVELS == 3 {
            p3_index
        } else if M::LEVELS == 4 {
            p4_index
        } else {
            unreachable!()
        };
        let start_idx = index_fn(start.into());
        let end_idx = index_fn(start.into() + size - 1) + 1;
        assert!(start_idx < ENTRY_COUNT);
        assert!(end_idx <= ENTRY_COUNT);
        for i in start_idx..end_idx {
            let entry = &mut dst_table[i];
            if !self.inner.borrowed_entries.set(i, true) && self.next_table(entry).is_ok() {
                self.dealloc_tree(entry.paddr(), 1);
            }
            *entry = src_table[i];
        }
    }

    /// Finishes modifications and flushes the TLB if necessary.
    pub fn finish(&mut self) {
        #[cfg(not(docsrs))]
        match &self.flush {
            ToFlush::None => {}
            ToFlush::Addresses(addrs) => {
                for vaddr in addrs.iter() {
                    M::flush_tlb_for(self.inner.root_paddr(), Some(*vaddr));
                }
            }
            ToFlush::Full => {
                M::flush_tlb_for(self.inner.root_paddr(), None);
            }
        }
        self.flush = ToFlush::None;
    }
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> Drop for PageTable64Mut<'_, M, PTE, H> {
    fn drop(&mut self) {
        self.finish();
    }
}
