//! LS2K1000 anonymous-page swap backed by the factory Linux swap file.
//!
//! Linux resolves `/swapfile-moss` while sda1 is mounted and passes its
//! physical extent map in `/chosen/moss,swap-extents`.  MOSS never parses or
//! writes sda1 filesystem metadata; it only performs page-sized I/O inside
//! those validated extents.  Logical page zero is permanently reserved for
//! the Linux swap header.

use alloc::{collections::VecDeque, sync::Arc, vec, vec::Vec};
use core::{
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};

use axerrno::{AxError, AxResult};
use axhal::mem::phys_to_virt;
use kspin::SpinNoIrq;
use memory_addr::{PAGE_SIZE_4K, PhysAddr};

use crate::{mm::AddrSpaceLock, task::AsThread};

const DT_PROPERTIES: [&str; 8] = [
    "moss,swap-extents-0",
    "moss,swap-extents-1",
    "moss,swap-extents-2",
    "moss,swap-extents-3",
    "moss,swap-extents-4",
    "moss,swap-extents-5",
    "moss,swap-extents-6",
    "moss,swap-extents-7",
];
const CELLS_PER_EXTENT: usize = 3;
const SWAP_HEADER_PAGES: usize = 1;
const LOW_FREE_BYTES: usize = 192 * 1024 * 1024;
const TARGET_FREE_BYTES: usize = 256 * 1024 * 1024;
const EMERGENCY_FREE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECLAIM_PAGES: usize = 8192;
const BACKGROUND_RECLAIM_PAGES: usize = 2048;
const BACKGROUND_LOCK_RETRY: Duration = Duration::from_millis(10);
const BACKGROUND_PROBE_RETRY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct SwapExtent {
    logical_page: usize,
    disk_lba: u64,
    pages: usize,
}

struct SwapState {
    extents: Vec<SwapExtent>,
    refs: Vec<u16>,
    cursor: usize,
    used: usize,
}

impl SwapState {
    fn disk_lba(&self, slot: usize) -> Option<u64> {
        let extent = self.extents.iter().find(|extent| {
            slot >= extent.logical_page && slot - extent.logical_page < extent.pages
        })?;
        Some(extent.disk_lba + ((slot - extent.logical_page) * 8) as u64)
    }

    fn reserve(&mut self) -> Option<usize> {
        if self.refs.len() <= SWAP_HEADER_PAGES || self.used + SWAP_HEADER_PAGES >= self.refs.len()
        {
            return None;
        }
        let usable = self.refs.len() - SWAP_HEADER_PAGES;
        for offset in 0..usable {
            let slot = SWAP_HEADER_PAGES
                + (self.cursor.saturating_sub(SWAP_HEADER_PAGES) + offset) % usable;
            if self.refs[slot] == 0 {
                self.refs[slot] = 1;
                self.used += 1;
                self.cursor = if slot + 1 < self.refs.len() {
                    slot + 1
                } else {
                    SWAP_HEADER_PAGES
                };
                return Some(slot);
            }
        }
        None
    }
}

static SWAP: SpinNoIrq<Option<SwapState>> = SpinNoIrq::new(None);
static SWAP_INS: AtomicU64 = AtomicU64::new(0);
static SWAP_OUTS: AtomicU64 = AtomicU64::new(0);
static SWAP_IO_ERRORS: AtomicU64 = AtomicU64::new(0);
static WORKING_SET_PROBES: AtomicU64 = AtomicU64::new(0);
static WORKING_SET_REFAULTS: AtomicU64 = AtomicU64::new(0);
static RECLAIM_ACTIVE: AtomicUsize = AtomicUsize::new(0);
static RECLAIM_WAIT: axtask::WaitQueue = axtask::WaitQueue::new();
static RECLAIM_WORK: axtask::WaitQueue = axtask::WaitQueue::new();
static RECLAIM_QUEUE: SpinNoIrq<VecDeque<Arc<AddrSpaceLock>>> = SpinNoIrq::new(VecDeque::new());
static RECLAIM_WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static BACKGROUND_RECLAIM_PASSES: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_RECLAIMED: AtomicU64 = AtomicU64::new(0);
static SYNCHRONOUS_RECLAIMED: AtomicU64 = AtomicU64::new(0);

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("validated DT cell"))
}

pub(crate) fn init() {
    let legacy = axhal::dtb::get_chosen_property("moss,swap-extents");
    let property_count = if legacy.is_some() {
        1
    } else {
        DT_PROPERTIES
            .iter()
            .take_while(|name| axhal::dtb::get_chosen_property(name).is_some())
            .count()
    };
    if property_count == 0 {
        warn!("MOSS_SWAP disabled: /chosen swap extent properties are absent");
        return;
    }

    let mut extents = Vec::new();
    let mut expected_logical = 0usize;
    for property_index in 0..property_count {
        let raw = legacy.unwrap_or_else(|| {
            axhal::dtb::get_chosen_property(DT_PROPERTIES[property_index])
                .expect("counted swap extent property disappeared")
        });
        if raw.is_empty() || raw.len() % (CELLS_PER_EXTENT * 4) != 0 {
            warn!(
                "MOSS_SWAP disabled: malformed extent property {} length {}",
                property_index,
                raw.len()
            );
            return;
        }
        extents.reserve(raw.len() / (CELLS_PER_EXTENT * 4));
        for cells in raw.chunks_exact(CELLS_PER_EXTENT * 4) {
            let logical_page = be_u32(&cells[0..4]) as usize;
            let disk_lba = be_u32(&cells[4..8]) as u64;
            let pages = be_u32(&cells[8..12]) as usize;
            if pages == 0 || logical_page != expected_logical {
                warn!(
                    "MOSS_SWAP disabled: non-contiguous extent logical={} expected={} pages={}",
                    logical_page, expected_logical, pages
                );
                return;
            }
            let Some(next_logical) = logical_page.checked_add(pages) else {
                warn!("MOSS_SWAP disabled: extent page count overflow");
                return;
            };
            let Some(last_lba) = disk_lba.checked_add((pages as u64).saturating_mul(8)) else {
                warn!("MOSS_SWAP disabled: extent LBA overflow");
                return;
            };
            if last_lba <= disk_lba {
                warn!("MOSS_SWAP disabled: invalid extent LBA range");
                return;
            }
            extents.push(SwapExtent {
                logical_page,
                disk_lba,
                pages,
            });
            expected_logical = next_logical;
        }
    }
    if expected_logical <= SWAP_HEADER_PAGES {
        warn!("MOSS_SWAP disabled: no usable pages after the Linux swap header");
        return;
    }

    let mut refs = vec![0; expected_logical];
    refs[0] = u16::MAX;
    let extent_count = extents.len();
    *SWAP.lock() = Some(SwapState {
        extents,
        refs,
        cursor: SWAP_HEADER_PAGES,
        used: 0,
    });
    info!(
        "MOSS_SWAP enabled: usable_pages={} usable_mib={} extents={} header_pages_reserved={}",
        expected_logical - SWAP_HEADER_PAGES,
        (expected_logical - SWAP_HEADER_PAGES) * PAGE_SIZE_4K / (1024 * 1024),
        extent_count,
        SWAP_HEADER_PAGES
    );
    start_background_reclaim_worker();
}

pub(crate) fn enabled() -> bool {
    SWAP.lock().is_some()
}

pub(crate) fn reserve_slot() -> Option<usize> {
    SWAP.lock().as_mut()?.reserve()
}

pub(crate) fn retain_slot(slot: usize) -> bool {
    let mut guard = SWAP.lock();
    let Some(state) = guard.as_mut() else {
        return false;
    };
    let Some(count) = state.refs.get_mut(slot) else {
        return false;
    };
    if *count == 0 || *count == u16::MAX {
        return false;
    }
    let Some(next) = count.checked_add(1) else {
        return false;
    };
    *count = next;
    true
}

pub(crate) fn release_slot(slot: usize) {
    let mut guard = SWAP.lock();
    let Some(state) = guard.as_mut() else {
        return;
    };
    let Some(count) = state.refs.get_mut(slot) else {
        warn!("MOSS_SWAP release ignored for invalid slot {slot}");
        return;
    };
    if *count == 0 || *count == u16::MAX {
        warn!("MOSS_SWAP release ignored for unowned slot {slot}");
        return;
    }
    *count -= 1;
    if *count == 0 {
        state.used = state.used.saturating_sub(1);
    }
}

fn slot_lba(slot: usize) -> AxResult<u64> {
    let guard = SWAP.lock();
    let state = guard.as_ref().ok_or(AxError::OperationNotSupported)?;
    if slot == 0 || state.refs.get(slot).copied().unwrap_or(0) == 0 {
        return Err(AxError::BadAddress);
    }
    state.disk_lba(slot).ok_or(AxError::BadAddress)
}

pub(crate) fn write_page(slot: usize, paddr: PhysAddr) -> AxResult {
    let lba = slot_lba(slot)?;
    let page = unsafe { core::slice::from_raw_parts(phys_to_virt(paddr).as_ptr(), PAGE_SIZE_4K) };
    axdriver::ls2k1000_raw_write(lba, page).map_err(|err| {
        SWAP_IO_ERRORS.fetch_add(1, Ordering::Relaxed);
        warn!("MOSS_SWAP write failed: slot={slot} lba={lba} error={err:?}");
        AxError::Io
    })?;
    SWAP_OUTS.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn read_page(slot: usize, paddr: PhysAddr) -> AxResult {
    let lba = slot_lba(slot)?;
    let page =
        unsafe { core::slice::from_raw_parts_mut(phys_to_virt(paddr).as_mut_ptr(), PAGE_SIZE_4K) };
    axdriver::ls2k1000_raw_read(lba, page).map_err(|err| {
        SWAP_IO_ERRORS.fetch_add(1, Ordering::Relaxed);
        warn!("MOSS_SWAP read failed: slot={slot} lba={lba} error={err:?}");
        AxError::Io
    })?;
    SWAP_INS.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn note_working_set_probes(pages: usize) {
    WORKING_SET_PROBES.fetch_add(pages as u64, Ordering::Relaxed);
}

pub(crate) fn note_working_set_refault() {
    WORKING_SET_REFAULTS.fetch_add(1, Ordering::Relaxed);
}

fn current_aspace() -> Option<Arc<AddrSpaceLock>> {
    let current = axtask::current();
    let thread = current.try_as_thread()?;
    Some(thread.proc_data.aspace())
}

fn queue_background_reclaim(aspace: Arc<AddrSpaceLock>) {
    let mut queue = RECLAIM_QUEUE.lock();
    if queue.iter().any(|queued| Arc::ptr_eq(queued, &aspace)) {
        return;
    }
    queue.push_back(aspace);
    drop(queue);
    RECLAIM_WORK.notify_one(true);
}

/// Runs one bounded address-space reclaim pass. `None` means another pass or
/// an address-space writer currently owns the relevant lock; callers must
/// retry instead of treating lock contention as an empty working set.
fn reclaim_aspace(aspace: &AddrSpaceLock, limit: usize, allow_recent: bool) -> Option<usize> {
    if limit == 0
        || !enabled()
        || RECLAIM_ACTIVE
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return None;
    }

    let reclaimed = aspace
        .try_lock()
        .map(|mut guard| guard.swap_out_anonymous_pages(limit, allow_recent));
    RECLAIM_ACTIVE.store(0, Ordering::Release);
    RECLAIM_WAIT.notify_all(false);
    reclaimed
}

/// Maintain the high watermark outside the allocating compiler thread. Small
/// bounded passes periodically release the address-space lock; a five-second
/// retry gives freshly cleared PTE.D entries their observation window. One
/// worker also prevents background swap from multiplying AHCI contention.
fn background_reclaim_worker() -> ! {
    loop {
        RECLAIM_WORK.wait_until(|| !RECLAIM_QUEUE.lock().is_empty());
        let Some(aspace) = RECLAIM_QUEUE.lock().pop_front() else {
            continue;
        };
        let allocator = axalloc::global_allocator();
        if allocator.available_pages() * PAGE_SIZE_4K >= TARGET_FREE_BYTES {
            continue;
        }

        let attempt = reclaim_aspace(&aspace, BACKGROUND_RECLAIM_PAGES, false);
        if let Some(reclaimed) = attempt {
            BACKGROUND_RECLAIM_PASSES.fetch_add(1, Ordering::Relaxed);
            BACKGROUND_RECLAIMED.fetch_add(reclaimed as u64, Ordering::Relaxed);
            // Swap traffic is maintenance, not proof that the foreground job
            // advances.  Let the watchdog's user-CPU/task-stage policy decide
            // whether the 20-minute progress lease should move forward.
        }

        if allocator.available_pages() * PAGE_SIZE_4K < TARGET_FREE_BYTES {
            queue_background_reclaim(aspace);
            match attempt {
                None => axtask::sleep(BACKGROUND_LOCK_RETRY),
                Some(0) => axtask::sleep(BACKGROUND_PROBE_RETRY),
                Some(_) => axtask::yield_now(),
            }
        }
    }
}

fn start_background_reclaim_worker() {
    if RECLAIM_WORKER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    axtask::spawn(|| background_reclaim_worker());
}

/// Queue proactive reclaim once RAM crosses the low watermark. The allocating
/// task continues while at least 64 MiB remains. Only the emergency band or an
/// actual allocation failure bypasses working-set protection synchronously.
pub(crate) fn reclaim_current_if_needed() -> usize {
    if !enabled() {
        return 0;
    }
    let allocator = axalloc::global_allocator();
    let available_bytes = allocator.available_pages() * PAGE_SIZE_4K;
    if available_bytes >= LOW_FREE_BYTES {
        return 0;
    }
    let Some(aspace) = current_aspace() else {
        return 0;
    };
    queue_background_reclaim(aspace);
    if available_bytes >= EMERGENCY_FREE_BYTES {
        return 0;
    }
    let wanted = (TARGET_FREE_BYTES.saturating_sub(available_bytes) / PAGE_SIZE_4K)
        .clamp(1, MAX_RECLAIM_PAGES);
    reclaim_current_inner(wanted, true)
}

pub(crate) fn reclaim_current(limit: usize) -> usize {
    reclaim_current_inner(limit, true)
}

fn reclaim_current_inner(limit: usize, allow_recent: bool) -> usize {
    let reclaimed = current_aspace()
        .and_then(|aspace| reclaim_aspace(&aspace, limit, allow_recent))
        .unwrap_or(0);
    SYNCHRONOUS_RECLAIMED.fetch_add(reclaimed as u64, Ordering::Relaxed);
    reclaimed
}

/// Avoid reporting a false OOM when the other core is already returning pages
/// to the global allocator. The predicate closes the notify-before-wait race.
pub(crate) fn wait_for_reclaim() {
    if RECLAIM_ACTIVE.load(Ordering::Acquire) != 0 && axtask::current_can_block() {
        RECLAIM_WAIT.wait_until(|| RECLAIM_ACTIVE.load(Ordering::Acquire) == 0);
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SwapStats {
    pub(crate) total_pages: usize,
    pub(crate) used_pages: usize,
    pub(crate) swap_in: u64,
    pub(crate) swap_out: u64,
    pub(crate) probes: u64,
    pub(crate) refaults: u64,
    pub(crate) io_errors: u64,
    pub(crate) background_passes: u64,
    pub(crate) background_reclaimed: u64,
    pub(crate) synchronous_reclaimed: u64,
}

pub(crate) fn stats() -> SwapStats {
    let guard = SWAP.lock();
    let (total_pages, used_pages) = guard
        .as_ref()
        .map(|state| (state.refs.len().saturating_sub(1), state.used))
        .unwrap_or_default();
    SwapStats {
        total_pages,
        used_pages,
        swap_in: SWAP_INS.load(Ordering::Relaxed),
        swap_out: SWAP_OUTS.load(Ordering::Relaxed),
        probes: WORKING_SET_PROBES.load(Ordering::Relaxed),
        refaults: WORKING_SET_REFAULTS.load(Ordering::Relaxed),
        io_errors: SWAP_IO_ERRORS.load(Ordering::Relaxed),
        background_passes: BACKGROUND_RECLAIM_PASSES.load(Ordering::Relaxed),
        background_reclaimed: BACKGROUND_RECLAIMED.load(Ordering::Relaxed),
        synchronous_reclaimed: SYNCHRONOUS_RECLAIMED.load(Ordering::Relaxed),
    }
}
