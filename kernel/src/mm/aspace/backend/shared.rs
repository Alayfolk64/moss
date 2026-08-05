use alloc::{sync::Arc, vec, vec::Vec};

use axerrno::{AxError, AxResult};
use axhal::paging::{MappingFlags, PageSize, PageTableMut, PagingError};
use kspin::SpinNoIrq;
use memory_addr::{MemoryAddr, PhysAddr, VirtAddr, VirtAddrRange};

use super::{
    AddrSpaceLock, Backend, BackendOps, alloc_frame, dealloc_frame, divide_page,
    finish_unmap_before_reclaim, pages_in,
};

pub struct SharedPages {
    phys_pages: SpinNoIrq<Vec<Option<PhysAddr>>>,
    pub size: PageSize,
}
impl SharedPages {
    pub fn new(size: usize, page_size: PageSize) -> Self {
        let num_pages = divide_page(size, page_size);
        Self {
            phys_pages: SpinNoIrq::new(vec![None; num_pages]),
            size: page_size,
        }
    }

    pub fn len(&self) -> usize {
        self.phys_pages.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, index: usize) -> Option<PhysAddr> {
        self.phys_pages.lock().get(index).copied().flatten()
    }

    fn get_or_alloc(&self, index: usize) -> AxResult<PhysAddr> {
        if let Some(paddr) = self.get(index) {
            return Ok(paddr);
        }

        let paddr = alloc_frame(true, self.size)?;
        let mut pages = self.phys_pages.lock();
        let Some(slot) = pages.get_mut(index) else {
            dealloc_frame(paddr, self.size);
            return Err(AxError::InvalidInput);
        };
        if let Some(existing) = *slot {
            dealloc_frame(paddr, self.size);
            Ok(existing)
        } else {
            *slot = Some(paddr);
            Ok(paddr)
        }
    }
}

impl Drop for SharedPages {
    fn drop(&mut self) {
        for frame in self.phys_pages.lock().iter().flatten() {
            dealloc_frame(*frame, self.size);
        }
    }
}

// FIXME: This implementation does not allow map or unmap partial ranges.
#[derive(Clone)]
pub struct SharedBackend {
    start: VirtAddr,
    pages: Arc<SharedPages>,
}
impl SharedBackend {
    pub fn pages(&self) -> &Arc<SharedPages> {
        &self.pages
    }

    pub(super) fn merge_compatible(&self, other: &Self) -> bool {
        self.start == other.start && Arc::ptr_eq(&self.pages, &other.pages)
    }

    fn page_index(&self, start: VirtAddr) -> usize {
        debug_assert!(start.is_aligned(self.pages.size));
        divide_page(start - self.start, self.pages.size)
    }
}

impl BackendOps for SharedBackend {
    fn page_size(&self) -> PageSize {
        self.pages.size
    }

    fn map(&self, range: VirtAddrRange, flags: MappingFlags, _pt: &mut PageTableMut) -> AxResult {
        debug!("Shared::map: {:?} {:?}", range, flags);
        Ok(())
    }

    fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
        debug!("Shared::unmap: {:?}", range);
        let result = (|| {
            for vaddr in pages_in(range, self.pages.size)? {
                match pt.unmap(vaddr) {
                    Ok(_) | Err(PagingError::NotMapped) => {}
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(())
        })();
        finish_unmap_before_reclaim(pt);
        result
    }

    fn populate(
        &self,
        range: VirtAddrRange,
        flags: MappingFlags,
        access_flags: MappingFlags,
        pt: &mut PageTableMut,
    ) -> AxResult<(usize, Option<super::PopulateCallback>)> {
        let mut pages = 0;
        let start_index = self.page_index(range.start);
        for (i, vaddr) in pages_in(range, self.pages.size)?.enumerate() {
            match pt.query(vaddr) {
                Ok((_paddr, page_flags, _)) if page_flags.contains(access_flags) => {
                    pages += 1;
                }
                Ok((paddr, _page_flags, _)) => {
                    pt.remap(vaddr, paddr, flags)?;
                    pages += 1;
                }
                Err(PagingError::NotMapped) => {
                    let paddr = self.pages.get_or_alloc(start_index + i)?;
                    pt.map(vaddr, paddr, self.pages.size, flags)?;
                    pages += 1;
                }
                Err(_) => return Err(AxError::BadAddress),
            }
        }
        Ok((pages, None))
    }

    fn clone_map(
        &self,
        _range: VirtAddrRange,
        _flags: MappingFlags,
        _old_pt: &mut PageTableMut,
        _new_pt: &mut PageTableMut,
        _new_aspace: &Arc<AddrSpaceLock>,
    ) -> AxResult<Backend> {
        Ok(Backend::Shared(self.clone()))
    }
}

impl Backend {
    pub fn new_shared(start: VirtAddr, pages: Arc<SharedPages>) -> Self {
        Self::Shared(SharedBackend { start, pages })
    }
}
