//! Memory mapping backends.
use alloc::{boxed::Box, sync::Arc};

use axalloc::{UsageKind, global_allocator};
use axerrno::{AxError, AxResult};
use axhal::{
    mem::{phys_to_virt, virt_to_phys},
    paging::{MappingFlags, PageSize, PageTable, PageTableMut},
};
use axsync::Mutex;
use enum_dispatch::enum_dispatch;
use kspin::SpinNoIrq;
use memory_addr::{DynPageIter, PAGE_SIZE_4K, PhysAddr, VirtAddr, VirtAddrRange};
use memory_set::MappingBackend;

mod cow;
mod file;
mod linear;
mod shared;

pub use self::shared::SharedPages;
use super::AddrSpace;

const LOCAL_FRAME_CACHE_CAPACITY: usize = 32;

struct LocalFrameCache {
    frames: [usize; LOCAL_FRAME_CACHE_CAPACITY],
    len: usize,
}

impl LocalFrameCache {
    const fn new() -> Self {
        Self {
            frames: [0; LOCAL_FRAME_CACHE_CAPACITY],
            len: 0,
        }
    }

    fn pop(&mut self) -> Option<PhysAddr> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(PhysAddr::from_usize(self.frames[self.len]))
    }

    fn push(&mut self, frame: PhysAddr) -> bool {
        if self.len == LOCAL_FRAME_CACHE_CAPACITY {
            return false;
        }
        self.frames[self.len] = frame.as_usize();
        self.len += 1;
        true
    }
}

static LOCAL_FRAME_CACHES: [SpinNoIrq<LocalFrameCache>; axconfig::plat::CPU_NUM] =
    [const { SpinNoIrq::new(LocalFrameCache::new()) }; axconfig::plat::CPU_NUM];

fn local_frame_cache() -> &'static SpinNoIrq<LocalFrameCache> {
    &LOCAL_FRAME_CACHES[axhal::percpu::this_cpu_id()]
}

fn divide_page(size: usize, page_size: PageSize) -> usize {
    assert!(page_size.is_aligned(size), "unaligned");
    size >> (page_size as usize).trailing_zeros()
}

fn alloc_frame(zeroed: bool, size: PageSize) -> AxResult<PhysAddr> {
    let page_size = size as usize;
    let num_pages = page_size / PAGE_SIZE_4K;
    let frame = if size == PageSize::Size4K {
        if let Some(frame) = local_frame_cache().lock().pop() {
            frame
        } else {
            let batch = match global_allocator().alloc_pages(
                LOCAL_FRAME_CACHE_CAPACITY,
                PAGE_SIZE_4K,
                UsageKind::VirtMem,
            ) {
                Ok(batch) => batch,
                Err(_) => {
                    let vaddr = global_allocator()
                        .alloc_pages(1, PAGE_SIZE_4K, UsageKind::VirtMem)
                        .map_err(|_| AxError::NoMemory)?;
                    let frame = virt_to_phys(VirtAddr::from(vaddr));
                    if zeroed {
                        unsafe {
                            core::ptr::write_bytes(
                                phys_to_virt(frame).as_mut_ptr(),
                                0,
                                PAGE_SIZE_4K,
                            )
                        };
                    }
                    return Ok(frame);
                }
            };
            let mut cache = local_frame_cache().lock();
            for index in 1..LOCAL_FRAME_CACHE_CAPACITY {
                cache.push(virt_to_phys(VirtAddr::from(batch + index * PAGE_SIZE_4K)));
            }
            virt_to_phys(VirtAddr::from(batch))
        }
    } else {
        let vaddr = VirtAddr::from(
            global_allocator()
                .alloc_pages(num_pages, page_size, UsageKind::VirtMem)
                .map_err(|_| AxError::NoMemory)?,
        );
        virt_to_phys(vaddr)
    };
    if zeroed {
        unsafe { core::ptr::write_bytes(phys_to_virt(frame).as_mut_ptr(), 0, page_size) };
    }
    Ok(frame)
}

fn dealloc_frame(frame: PhysAddr, align: PageSize) {
    if align == PageSize::Size4K && local_frame_cache().lock().push(frame) {
        return;
    }
    let vaddr = phys_to_virt(frame);
    let page_size: usize = align.into();
    let num_pages = page_size / PAGE_SIZE_4K;
    global_allocator().dealloc_pages(vaddr.as_usize(), num_pages, UsageKind::VirtMem);
}

fn pages_in(range: VirtAddrRange, align: PageSize) -> AxResult<DynPageIter<VirtAddr>> {
    DynPageIter::new(range.start, range.end, align as usize).ok_or(AxError::InvalidInput)
}

type PopulateCallback = Box<dyn FnOnce(&mut AddrSpace)>;

#[enum_dispatch]
pub trait BackendOps {
    /// Returns the page size of the backend.
    fn page_size(&self) -> PageSize;

    /// Map a memory region.
    fn map(&self, range: VirtAddrRange, flags: MappingFlags, pt: &mut PageTableMut) -> AxResult;

    /// Unmap a memory region.
    fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult;

    /// Discard resident pages while retaining the virtual memory area.
    fn discard(&self, _range: VirtAddrRange, _pt: &mut PageTableMut) -> AxResult {
        Err(AxError::OperationNotSupported)
    }

    /// Called before a memory region is protected.
    fn on_protect(
        &self,
        _range: VirtAddrRange,
        _new_flags: MappingFlags,
        _pt: &mut PageTableMut,
    ) -> AxResult {
        Ok(())
    }

    /// Populate a memory region and return how many pages now satisfy
    /// `access_flags`.
    ///
    /// If another thread has already mapped the page with sufficient permissions,
    /// treat it as populated.
    fn populate(
        &self,
        _range: VirtAddrRange,
        _flags: MappingFlags,
        _access_flags: MappingFlags,
        _pt: &mut PageTableMut,
    ) -> AxResult<(usize, Option<PopulateCallback>)> {
        Ok((0, None))
    }

    /// Duplicates this mapping for use in a different page table.
    ///
    /// This differs from `clone`, which is designed for splitting a mapping
    /// within the same table.
    ///
    /// [`BackendOps::map`] will be latter called to the returned backend.
    fn clone_map(
        &self,
        range: VirtAddrRange,
        flags: MappingFlags,
        old_pt: &mut PageTableMut,
        new_pt: &mut PageTableMut,
        new_aspace: &Arc<Mutex<AddrSpace>>,
    ) -> AxResult<Backend>;
}

/// A unified enum type for different memory mapping backends.
#[derive(Clone)]
#[enum_dispatch(BackendOps)]
pub enum Backend {
    Linear(linear::LinearBackend), // 线性映射，虚拟地址和物理地址有固定偏移关系
    Cow(cow::CowBackend),          // Copy-on-write，写时复制
    Shared(shared::SharedBackend), // 共享内存
    File(file::FileBackend),       // 文件映射
}

impl MappingBackend for Backend {
    type Addr = VirtAddr;
    type Flags = MappingFlags;
    type PageTable = PageTable;

    fn map(&self, start: VirtAddr, size: usize, flags: MappingFlags, pt: &mut PageTable) -> bool {
        let range = VirtAddrRange::from_start_size(start, size);
        let mut modify = pt.modify();
        if let Err(err) = BackendOps::map(self, range, flags, &mut modify) {
            warn!("Failed to map area: {:?}", err);
            false
        } else {
            true
        }
    }

    fn unmap(&self, start: VirtAddr, size: usize, pt: &mut PageTable) -> bool {
        let range = VirtAddrRange::from_start_size(start, size);
        let mut modify = pt.modify();
        if let Err(err) = BackendOps::unmap(self, range, &mut modify) {
            warn!("Failed to unmap area: {:?}", err);
            false
        } else {
            true
        }
    }

    fn protect(
        &self,
        start: Self::Addr,
        size: usize,
        new_flags: Self::Flags,
        pt: &mut Self::PageTable,
    ) -> bool {
        let range = VirtAddrRange::from_start_size(start, size);
        let mut cursor = pt.modify();
        if let Err(err) = BackendOps::on_protect(self, range, new_flags, &mut cursor) {
            warn!("Failed to protect area: {:?}", err);
            return false;
        }
        cursor.protect_region(start, size, new_flags).is_ok()
    }
}
