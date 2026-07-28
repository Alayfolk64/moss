use strum::VariantArray;

use super::{GlobalAllocator, UsageKind};

const PAGE_SIZE: usize = 4096;
const BATCH_SCALE_MAX: u8 = 5;
const MIN_FREE_KBYTES: usize = 128;
const MAX_FREE_KBYTES: usize = 256 * 1024;
const WATERMARK_SCALE_FACTOR: usize = 10;
const PERCPU_HIGH_FRACTION: usize = 8;

pub(crate) const PCP_CPU_SLOTS: usize = 64;

#[derive(Clone, Copy)]
struct PcpLimits {
    batch: usize,
    high_min: usize,
    high_max: usize,
}

/// A cache of free order-0 pages for one CPU.
///
/// This follows the central Linux PCP rules:
///
/// - `batch` is derived from the managed-memory size;
/// - `high` starts from the low watermark split over the active CPUs;
/// - repeated allocation misses grow both the refill batch and `high`;
/// - consecutive frees are returned to the global allocator in one batch;
/// - pressure-driven draining shrinks `high` back to `high_min`.
///
/// Linux links free pages through `struct page`. Moss has no equivalent page
/// metadata, so an unused page stores the next-list address in its first
/// machine word. The word belongs to the allocator while the page is free and
/// is never observed by its next owner.
pub(crate) struct PerCpuPageCache {
    heads: [usize; UsageKind::VARIANTS.len()],
    kind_counts: [usize; UsageKind::VARIANTS.len()],
    count: usize,
    batch: usize,
    high: usize,
    high_min: usize,
    high_max: usize,
    alloc_factor: u8,
    free_count: usize,
    configured_total_pages: usize,
    configured_cpu_count: usize,
    next_drain: usize,
}

impl PerCpuPageCache {
    pub(crate) const fn new() -> Self {
        Self {
            heads: [0; UsageKind::VARIANTS.len()],
            kind_counts: [0; UsageKind::VARIANTS.len()],
            count: 0,
            batch: 1,
            high: 1,
            high_min: 1,
            high_max: 1,
            alloc_factor: 0,
            free_count: 0,
            configured_total_pages: 0,
            configured_cpu_count: 0,
            next_drain: 0,
        }
    }

    /// Takes a cached page without entering a refill or drain path.
    ///
    /// The caller may use an IRQ-only lock because this path neither schedules
    /// nor acquires the global page allocator. Configuration and misses are
    /// deliberately left to [`Self::allocate`], whose caller also disables
    /// preemption across nested allocator locks.
    #[inline]
    pub(crate) fn try_allocate(
        &mut self,
        total_pages: usize,
        cpu_count: usize,
        kind: UsageKind,
    ) -> Option<usize> {
        if self.configured_total_pages != total_pages
            || self.configured_cpu_count != cpu_count.max(1)
            || self.heads[kind as usize] == 0
        {
            return None;
        }

        self.free_count >>= 1;
        self.pop(kind)
    }

    #[inline]
    pub(crate) fn allocate(
        &mut self,
        allocator: &GlobalAllocator,
        total_pages: usize,
        cpu_count: usize,
        kind: UsageKind,
    ) -> Option<usize> {
        if self.configured_total_pages != total_pages
            || self.configured_cpu_count != cpu_count.max(1)
        {
            self.update_limits(allocator, total_pages, cpu_count);
        }
        self.free_count >>= 1;

        if let Some(page) = self.pop(kind) {
            return Some(page);
        }

        self.refill_and_pop(allocator, kind)
    }

    #[cold]
    #[inline(never)]
    fn refill_and_pop(&mut self, allocator: &GlobalAllocator, kind: UsageKind) -> Option<usize> {
        self.refill(allocator, kind);
        self.pop(kind)
    }

    #[inline]
    fn pop(&mut self, kind: UsageKind) -> Option<usize> {
        let index = kind as usize;
        let page = self.heads[index];
        if page == 0 {
            return None;
        }

        self.heads[index] = unsafe { read_next(page) };
        self.kind_counts[index] -= 1;
        self.count -= 1;
        Some(page)
    }

    pub(crate) fn release(
        &mut self,
        allocator: &GlobalAllocator,
        total_pages: usize,
        cpu_count: usize,
        page: usize,
        kind: UsageKind,
    ) {
        self.update_limits(allocator, total_pages, cpu_count);
        self.alloc_factor >>= 1;

        let index = kind as usize;
        unsafe { write_next(page, self.heads[index]) };
        self.heads[index] = page;
        self.kind_counts[index] += 1;
        self.count += 1;
        let max_free_count = self
            .batch
            .checked_shl(BATCH_SCALE_MAX.into())
            .unwrap_or(usize::MAX);
        self.free_count = (self.free_count + 1).min(max_free_count);

        let drain_high = self.high;
        if self.count >= drain_high && self.high_min != self.high_max {
            let needed = self.free_count.saturating_add(self.batch);
            self.high = self.high.max(needed).min(self.high_max);
        }

        if self.count >= drain_high {
            let drain = if drain_high < self.batch {
                1
            } else {
                self.free_count
                    .clamp(self.batch, drain_high.saturating_sub(self.batch))
            };
            self.drain(allocator, drain);
        }
    }

    pub(crate) fn drain_all(&mut self, allocator: &GlobalAllocator) -> usize {
        let count = self.count;
        self.drain(allocator, count);
        self.high = self.high_min;
        self.alloc_factor = 0;
        self.free_count = 0;
        count
    }

    pub(crate) const fn count(&self) -> usize {
        self.count
    }

    fn update_limits(&mut self, allocator: &GlobalAllocator, total_pages: usize, cpu_count: usize) {
        let cpu_count = cpu_count.max(1);
        if self.configured_total_pages == total_pages && self.configured_cpu_count == cpu_count {
            return;
        }

        let limits = calculate_limits(total_pages, cpu_count);
        self.batch = limits.batch;
        self.high_min = limits.high_min;
        self.high_max = limits.high_max;
        self.high = self.high.clamp(self.high_min, self.high_max);
        self.configured_total_pages = total_pages;
        self.configured_cpu_count = cpu_count;

        if self.count > self.high {
            self.drain(allocator, self.count - self.high);
        }
    }

    fn refill(&mut self, allocator: &GlobalAllocator, kind: UsageKind) {
        let base_batch = self.batch;
        let mut batch = base_batch
            .checked_shl(self.alloc_factor.into())
            .unwrap_or(usize::MAX);

        if self.high_min != self.high_max {
            self.high = self.high.saturating_add(batch).min(self.high_max);
        }

        let max_refill = self
            .high
            .saturating_sub(self.count)
            .saturating_sub(base_batch)
            .max(base_batch);
        if batch <= max_refill && self.alloc_factor < BATCH_SCALE_MAX {
            self.alloc_factor += 1;
        }
        batch = batch.min(max_refill);

        let index = kind as usize;
        debug_assert_eq!(self.heads[index], 0);
        debug_assert_eq!(self.kind_counts[index], 0);
        let (head, allocated) = allocator.allocate_page_list(batch, kind);
        self.heads[index] = head;
        self.kind_counts[index] += allocated;
        self.count += allocated;

        if allocated < batch {
            self.high = self.high_min;
            self.alloc_factor = 0;
        }
    }

    fn drain(&mut self, allocator: &GlobalAllocator, pages: usize) {
        let pages = pages.min(self.count);
        if pages == 0 {
            return;
        }

        let mut drain_heads = [0usize; UsageKind::VARIANTS.len()];
        let mut drain_counts = [0usize; UsageKind::VARIANTS.len()];
        let mut remaining = pages;
        let first_kind = self.next_drain;
        let mut last_kind = first_kind;

        for offset in 0..UsageKind::VARIANTS.len() {
            if remaining == 0 {
                break;
            }
            let index = (first_kind + offset) % UsageKind::VARIANTS.len();
            let take = remaining.min(self.kind_counts[index]);
            if take == 0 {
                continue;
            }

            let head = self.heads[index];
            let mut tail = head;
            for _ in 1..take {
                tail = unsafe { read_next(tail) };
            }
            self.heads[index] = unsafe { read_next(tail) };
            unsafe { write_next(tail, 0) };
            self.kind_counts[index] -= take;
            self.count -= take;
            remaining -= take;
            drain_heads[index] = head;
            drain_counts[index] = take;
            last_kind = index;
        }

        debug_assert_eq!(remaining, 0);
        self.next_drain = (last_kind + 1) % UsageKind::VARIANTS.len();
        allocator.deallocate_page_lists(&drain_heads, &drain_counts);
    }
}

fn calculate_limits(total_pages: usize, cpu_count: usize) -> PcpLimits {
    // Linux zone_batchsize(): about 0.1% of memory or 1 MiB, divided by
    // four, then rounded down to 2^n - 1 to avoid page-color aliasing.
    let mut batch = ((total_pages >> 10).min(1024 * 1024 / PAGE_SIZE) / 4).max(1);
    batch = floor_power_of_two(batch.saturating_add(batch / 2))
        .saturating_sub(1)
        .max(1);

    // Linux's default min_free_kbytes and low watermark for one managed zone.
    let lowmem_kbytes = total_pages.saturating_mul(PAGE_SIZE / 1024);
    let min_free_kbytes =
        integer_sqrt(lowmem_kbytes.saturating_mul(16)).clamp(MIN_FREE_KBYTES, MAX_FREE_KBYTES);
    let min_pages = min_free_kbytes / (PAGE_SIZE / 1024);
    let watermark_delta =
        (min_pages / 4).max(total_pages.saturating_mul(WATERMARK_SCALE_FACTOR) / 10_000);
    let low_watermark = min_pages.saturating_add(watermark_delta);

    let cpu_count = cpu_count.max(1);
    let high_min = (low_watermark / cpu_count)
        .max(batch.saturating_mul(4))
        .max(1);
    let high_max = (total_pages / PERCPU_HIGH_FRACTION / cpu_count).max(high_min);

    PcpLimits {
        batch,
        high_min,
        high_max,
    }
}

unsafe fn read_next(page: usize) -> usize {
    debug_assert_ne!(page, 0);
    unsafe { (page as *const usize).read() }
}

unsafe fn write_next(page: usize, next: usize) {
    debug_assert_ne!(page, 0);
    unsafe { (page as *mut usize).write(next) }
}

fn floor_power_of_two(value: usize) -> usize {
    if value == 0 {
        return 0;
    }
    1usize << (usize::BITS - value.leading_zeros() - 1)
}

fn integer_sqrt(value: usize) -> usize {
    if value < 2 {
        return value;
    }

    let bits = usize::BITS - value.leading_zeros();
    let mut root = 1usize << bits.div_ceil(2);
    loop {
        let next = (root + value / root) / 2;
        if next >= root {
            return root;
        }
        root = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_like_limits_for_eight_gibibytes() {
        let limits = calculate_limits(8 * 1024 * 1024 * 1024 / PAGE_SIZE, 8);
        assert_eq!(limits.batch, 63);
        assert_eq!(limits.high_min, 624);
        assert_eq!(limits.high_max, 32_768);
    }

    #[test]
    fn small_machine_limits_remain_ordered() {
        let limits = calculate_limits(16 * 1024 * 1024 / PAGE_SIZE, 8);
        assert!(limits.batch >= 1);
        assert!(limits.batch <= limits.high_min);
        assert!(limits.high_min <= limits.high_max);
    }
}
