use alloc::collections::BTreeMap;
use core::fmt;
use core::ops::Bound::{Excluded, Unbounded};

use memory_addr::{AddrRange, MemoryAddr};

use crate::{MappingBackend, MappingError, MappingResult, MemoryArea};

/// A container that maintains memory mappings ([`MemoryArea`]).
pub struct MemorySet<B: MappingBackend, M = ()> {
    areas: BTreeMap<B::Addr, MemoryArea<B, M>>,
}

/// Exact structural work performed by [`MemorySet::protect`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtectStats {
    /// VMAs checked while validating complete range coverage and permissions.
    pub validated: usize,
    /// VMAs revisited during the mutation pass.
    pub visited: usize,
    /// VMAs whose current flags actually changed.
    pub changed: usize,
    /// VMAs already carrying the requested flags and metadata.
    pub unchanged: usize,
    /// New VMA boundaries created at the requested range edges.
    pub split: usize,
    /// Adjacent compatible VMAs coalesced after the change.
    pub merged: usize,
    /// Number of VMAs in this memory set after the operation.
    pub output: usize,
}

impl<B: MappingBackend, M: Clone> MemorySet<B, M> {
    /// Creates a new memory set.
    pub const fn new() -> Self {
        Self {
            areas: BTreeMap::new(),
        }
    }

    /// Returns the number of memory areas in the memory set.
    pub fn len(&self) -> usize {
        self.areas.len()
    }

    /// Returns `true` if the memory set contains no memory areas.
    pub fn is_empty(&self) -> bool {
        self.areas.is_empty()
    }

    /// Returns the iterator over all memory areas.
    pub fn iter(&self) -> impl Iterator<Item = &MemoryArea<B, M>> {
        self.areas.values()
    }

    /// Returns whether the given address range overlaps with any existing area.
    pub fn overlaps(&self, range: AddrRange<B::Addr>) -> bool {
        if let Some((_, before)) = self.areas.range(..range.start).last() {
            if before.va_range().overlaps(range) {
                return true;
            }
        }
        if let Some((_, after)) = self.areas.range(range.start..).next() {
            if after.va_range().overlaps(range) {
                return true;
            }
        }
        false
    }

    /// Finds the memory area that contains the given address.
    pub fn find(&self, addr: B::Addr) -> Option<&MemoryArea<B, M>> {
        let candidate = self.areas.range(..=addr).last().map(|(_, a)| a);
        candidate.filter(|a| a.va_range().contains(addr))
    }

    /// Finds a free area that can accommodate the given size.
    ///
    /// The search starts from the given `hint` address, and the area should be
    /// within the given `limit` range.
    ///
    /// # Notes
    /// The `align` parameter specifies the alignment of the start address and
    /// the size of the area. The start address of the resulting area will
    /// be aligned to this value. Also, the size of the area must be a multiple
    /// of this value.
    ///
    /// # Returns
    /// Returns the start address of the free area. Returns `None` if no such
    /// area is found.
    pub fn find_free_area(
        &self,
        hint: B::Addr,
        size: usize,
        limit: AddrRange<B::Addr>,
        align: usize,
    ) -> Option<B::Addr> {
        if size % align != 0 {
            // size must be a multiple of align.
            return None;
        }
        // brute force: try each area's end address as the start.
        let mut last_end: <B as MappingBackend>::Addr = hint.max(limit.start).align_up(align);
        if let Some((_, area)) = self.areas.range(..last_end).last() {
            last_end = last_end.max(area.end()).align_up(align);
        }
        for (&addr, area) in self.areas.range(last_end..) {
            if last_end.checked_add(size).is_some_and(|end| end <= addr) {
                return Some(last_end);
            }
            last_end = area.end().align_up(align);
        }
        if last_end
            .checked_add(size)
            .is_some_and(|end| end <= limit.end)
        {
            Some(last_end)
        } else {
            None
        }
    }

    /// Add a new memory mapping.
    ///
    /// The mapping is represented by a [`MemoryArea`].
    ///
    /// If the new area overlaps with any existing area, the behavior is
    /// determined by the `unmap_overlap` parameter. If it is `true`, the
    /// overlapped regions will be unmapped first. Otherwise, it returns an
    /// error.
    pub fn map(
        &mut self,
        area: MemoryArea<B, M>,
        page_table: &mut B::PageTable,
        unmap_overlap: bool,
    ) -> MappingResult {
        if area.va_range().is_empty() {
            return Err(MappingError::InvalidParam);
        }

        if self.overlaps(area.va_range()) {
            if unmap_overlap {
                self.unmap(area.start(), area.size(), page_table)?;
            } else {
                return Err(MappingError::AlreadyExists);
            }
        }

        area.map_area(page_table)?;
        assert!(self.areas.insert(area.start(), area).is_none());
        Ok(())
    }

    /// Remove memory mappings within the given address range.
    ///
    /// All memory areas that are fully contained in the range will be removed
    /// directly. If the area intersects with the boundary, it will be shrinked.
    /// If the unmapped range is in the middle of an existing area, it will be
    /// split into two areas.
    pub fn unmap(
        &mut self,
        start: B::Addr,
        size: usize,
        page_table: &mut B::PageTable,
    ) -> MappingResult {
        let range =
            AddrRange::try_from_start_size(start, size).ok_or(MappingError::InvalidParam)?;
        if range.is_empty() {
            return Ok(());
        }

        let end = range.end;

        // Unmap entire areas that are contained by the range.
        self.areas.retain(|_, area| {
            if area.va_range().contained_in(range) {
                area.unmap_area(page_table).unwrap();
                false
            } else {
                true
            }
        });

        // Shrink right if the area intersects with the left boundary.
        if let Some((&before_start, before)) = self.areas.range_mut(..start).last() {
            let before_end = before.end();
            if before_end > start {
                if before_end <= end {
                    // the unmapped area is at the end of `before`.
                    before.shrink_right(start.sub_addr(before_start), page_table)?;
                } else {
                    // the unmapped area is in the middle `before`, need to split.
                    let right_part = before.split(end).unwrap();
                    before.shrink_right(start.sub_addr(before_start), page_table)?;
                    assert_eq!(right_part.start().into(), Into::<usize>::into(end));
                    self.areas.insert(end, right_part);
                }
            }
        }

        // Shrink left if the area intersects with the right boundary.
        if let Some((&after_start, after)) = self.areas.range_mut(start..).next() {
            let after_end = after.end();
            if after_start < end {
                // the unmapped area is at the start of `after`.
                let mut new_area = self.areas.remove(&after_start).unwrap();
                new_area.shrink_left(after_end.sub_addr(end), page_table)?;
                assert_eq!(new_area.start().into(), Into::<usize>::into(end));
                self.areas.insert(end, new_area);
            }
        }

        Ok(())
    }

    /// Remove all memory areas and the underlying mappings.
    pub fn clear(&mut self, page_table: &mut B::PageTable) -> MappingResult {
        for (_, area) in self.areas.iter() {
            area.unmap_area(page_table)?;
        }
        self.areas.clear();
        Ok(())
    }

    /// Change mapping flags in a completely mapped address range.
    ///
    /// The validation pass starts at the VMA containing `start` and visits
    /// only overlapping VMAs. It rejects holes before modifying the page table.
    /// `update_flags` also validates caller-defined metadata and returns
    /// [`None`] for an unchanged VMA. `can_merge` decides whether adjacent VMAs
    /// describe the same mapping and may be coalesced after the update.
    pub fn protect(
        &mut self,
        start: B::Addr,
        size: usize,
        update_flags: impl Fn(&MemoryArea<B, M>) -> MappingResult<Option<(B::Flags, M)>>,
        can_merge: impl Fn(&MemoryArea<B, M>, &MemoryArea<B, M>) -> bool,
        page_table: &mut B::PageTable,
    ) -> MappingResult<ProtectStats> {
        let end = start.checked_add(size).ok_or(MappingError::InvalidParam)?;
        if start == end {
            return Ok(ProtectStats {
                output: self.areas.len(),
                ..ProtectStats::default()
            });
        }

        // Validate the entire interval before changing either VMAs or PTEs.
        // Each lookup starts at the previous area's end, so no VMA before the
        // requested range is scanned.
        let mut stats = ProtectStats::default();
        let mut needs_update = false;
        let mut cursor = start;
        while cursor < end {
            let area = self.find(cursor).ok_or(MappingError::NotMapped)?;
            stats.validated += 1;
            if update_flags(area)?.is_some() {
                needs_update = true;
            } else {
                stats.unchanged += 1;
            }
            cursor = area.end().min(end);
        }
        if !needs_update {
            stats.output = self.areas.len();
            return Ok(stats);
        }

        // Revisit only the VMAs proven to cover the interval. Removing one VMA
        // at a time avoids allocating a temporary key vector on the hot path.
        cursor = start;
        while cursor < end {
            let (area_start, affected_end, update) = {
                let (area_start, area) = self
                    .areas
                    .range(..=cursor)
                    .next_back()
                    .filter(|(_, area)| area.end() > cursor)
                    .ok_or(MappingError::NotMapped)?;
                (*area_start, area.end().min(end), update_flags(area)?)
            };
            stats.visited += 1;
            let Some((new_flags, new_metadata)) = update else {
                // Preserve the original VMA exactly. In particular, do not
                // manufacture range-boundary splits for a no-op mprotect.
                cursor = affected_end;
                continue;
            };

            // Complete the only fallible mutation while the original VMA is
            // still present. If the backend rejects the page-table update,
            // the VMA tree remains structurally unchanged.
            self.areas
                .get(&area_start)
                .expect("validated VMA disappeared while MemorySet is exclusively borrowed")
                .protect_subrange(
                    cursor,
                    affected_end.wrapping_sub_addr(cursor),
                    new_flags,
                    page_table,
                )?;

            // From this point onward the operation only reshapes in-memory
            // metadata and cannot return an error.
            let mut area = self
                .areas
                .remove(&area_start)
                .expect("validated VMA disappeared while MemorySet is exclusively borrowed");
            if area.start() < cursor {
                let affected = area
                    .split(cursor)
                    .expect("cursor must be strictly inside the validated VMA");
                self.areas.insert(area.start(), area);
                area = affected;
                stats.split += 1;
            }

            let trailing = if affected_end < area.end() {
                stats.split += 1;
                area.split(affected_end)
            } else {
                None
            };

            area.set_flags(new_flags);
            area.set_metadata(new_metadata);
            stats.changed += 1;

            let affected_start = area.start();
            self.areas.insert(affected_start, area);
            if let Some(trailing) = trailing {
                self.areas.insert(trailing.start(), trailing);
            }

            stats.merged += self.merge_at(affected_start, &can_merge);
            cursor = affected_end;
        }

        stats.output = self.areas.len();
        Ok(stats)
    }

    fn merge_at(
        &mut self,
        area_start: B::Addr,
        can_merge: &impl Fn(&MemoryArea<B, M>, &MemoryArea<B, M>) -> bool,
    ) -> usize {
        let mut merged = 0;
        let mut current_start = area_start;

        if let Some(previous_start) = self
            .areas
            .range(..current_start)
            .next_back()
            .and_then(|(previous_start, previous)| {
                let current = self.areas.get(&current_start)?;
                (previous.end() == current.start() && can_merge(previous, current))
                    .then_some(*previous_start)
            })
        {
            let current = self
                .areas
                .remove(&current_start)
                .expect("merge candidate disappeared");
            self.areas
                .get_mut(&previous_start)
                .expect("merge predecessor disappeared")
                .set_end(current.end());
            current_start = previous_start;
            merged += 1;
        }

        loop {
            let Some(next_start) = self
                .areas
                .range((Excluded(current_start), Unbounded))
                .next()
                .and_then(|(next_start, next)| {
                    let current = self.areas.get(&current_start)?;
                    (current.end() == next.start() && can_merge(current, next))
                        .then_some(*next_start)
                })
            else {
                break;
            };
            let next = self
                .areas
                .remove(&next_start)
                .expect("merge successor disappeared");
            self.areas
                .get_mut(&current_start)
                .expect("merge current VMA disappeared")
                .set_end(next.end());
            merged += 1;
        }

        merged
    }
}

impl<B: MappingBackend, M: Clone> Default for MemorySet<B, M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: MappingBackend, M: Clone> fmt::Debug for MemorySet<B, M>
where
    B::Addr: fmt::Debug,
    B::Flags: fmt::Debug,
    M: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_list().entries(self.areas.values()).finish()
    }
}
