use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Weak},
    vec::Vec,
};
#[cfg(feature = "times")]
use core::sync::atomic::AtomicU8;
use core::{
    ops::Range,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    task::Context,
    time::Duration,
};

use axalloc::{UsageKind, global_allocator};
use axfs_ng_vfs::{
    FileNode, Location, Metadata, MetadataUpdate, NodeFlags, NodePermission, NodeType, VfsError,
    VfsResult, flush_inode_metadata_caches, path::Path,
};
use axhal::mem::{PhysAddr, VirtAddr, virt_to_phys};
use axio::{SeekFrom, prelude::*};
use axpoll::{IoEvents, Pollable};
use axsync::{Mutex, MutexGuard};
use axtask::WaitQueue;
use intrusive_collections::{LinkedList, LinkedListAtomicLink, intrusive_adapter};
use kspin::SpinNoIrq;
use spin::RwLock;

use super::FsContext;

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct FileFlags: u8 {
        const READ = 1;
        const WRITE = 2;
        const EXECUTE = 4;
        const APPEND = 8;
        const PATH = 16;
    }
}

/// Results returned by [`OpenOptions::open`].
pub enum OpenResult {
    File(File),
    Dir(Location),
}

impl OpenResult {
    pub fn into_file(self) -> VfsResult<File> {
        match self {
            Self::File(file) => Ok(file),
            Self::Dir(_) => Err(VfsError::IsADirectory),
        }
    }

    pub fn into_dir(self) -> VfsResult<Location> {
        match self {
            Self::Dir(dir) => Ok(dir),
            Self::File(_) => Err(VfsError::NotADirectory),
        }
    }

    pub fn into_location(self) -> Location {
        match self {
            Self::File(file) => file.location().clone(),
            Self::Dir(dir) => dir,
        }
    }
}

/// Options and flags which can be used to configure how a file is opened.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    // generic
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    directory: bool,
    no_follow: bool,
    direct: bool,
    user: Option<(u32, u32)>,
    path: bool,
    node_type: NodeType,
    // system-specific
    mode: u32,
}

impl OpenOptions {
    /// Creates a blank new set of options ready for configuration.
    pub fn new() -> Self {
        Self {
            // generic
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            directory: false,
            no_follow: false,
            direct: false,
            user: None,
            path: false,
            node_type: NodeType::RegularFile,
            // system-specific
            mode: 0o666,
        }
    }

    /// Sets the option for read access.
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    /// Sets the option for write access.
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    /// Sets the option for the append mode.
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    /// Sets the option for truncating a previous file.
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    /// Sets the option to create a new file, or open it if it already exists.
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    /// Sets the option to create a new file, failing if it already exists.
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    /// Sets the option to open directory instead.
    pub fn directory(&mut self, directory: bool) -> &mut Self {
        self.directory = directory;
        self
    }

    /// Sets the option to not follow symlinks.
    pub fn no_follow(&mut self, no_follow: bool) -> &mut Self {
        self.no_follow = no_follow;
        self
    }

    /// Sets the option to open the file with direct I/O.\
    pub fn direct(&mut self, direct: bool) -> &mut Self {
        self.direct = direct;
        self
    }

    /// Sets the user and group id to open the file with.
    pub fn user(&mut self, uid: u32, gid: u32) -> &mut Self {
        self.user = Some((uid, gid));
        self
    }

    /// Sets the option for path only access.
    pub fn path(&mut self, path: bool) -> &mut Self {
        self.path = path;
        self
    }

    /// Sets the node type for the file.
    ///
    /// This will only be used if the file is created.
    pub fn node_type(&mut self, node_type: NodeType) -> &mut Self {
        self.node_type = node_type;
        self
    }

    /// Sets the mode bits that a new file will be created with.
    pub fn mode(&mut self, mode: u32) -> &mut Self {
        self.mode = mode;
        self
    }

    fn _open(&self, loc: Location) -> VfsResult<OpenResult> {
        let flags = self.to_flags()?;

        if self.directory {
            if flags.contains(FileFlags::WRITE) {
                return Err(VfsError::IsADirectory);
            }
            loc.check_is_dir()?;
        }
        if self.truncate && loc.is_dir() {
            return Err(VfsError::IsADirectory);
        }
        Ok(if loc.is_dir() {
            OpenResult::Dir(loc)
        } else {
            // TODO(mivik): is this correct?
            let non_cacheable_type = matches!(
                loc.node_type(),
                NodeType::CharacterDevice | NodeType::Fifo | NodeType::Socket
            );

            let direct = non_cacheable_type
                || self.path
                || self.direct
                || loc.flags().contains(NodeFlags::NON_CACHEABLE);
            let backend = if !direct || loc.flags().contains(NodeFlags::ALWAYS_CACHE) {
                FileBackend::new_cached(loc)
            } else {
                FileBackend::new_direct(loc)
            };
            if self.truncate {
                backend.set_len(0)?;
            }
            OpenResult::File(File::new(backend, flags))
        })
    }

    pub fn open_loc(&self, loc: Location) -> VfsResult<OpenResult> {
        if !self.is_valid() {
            return Err(VfsError::InvalidInput);
        }
        self._open(loc)
    }

    pub fn open(&self, context: &FsContext, path: impl AsRef<Path>) -> VfsResult<OpenResult> {
        if !self.is_valid() {
            return Err(VfsError::InvalidInput);
        }

        let loc = match context.resolve_parent(path.as_ref()) {
            Ok((parent, name)) => {
                let mut loc = parent.open_file(
                    &name,
                    &axfs_ng_vfs::OpenOptions {
                        create: self.create,
                        create_new: self.create_new,
                        node_type: self.node_type,
                        permission: NodePermission::from_bits_truncate(self.mode as _),
                        user: self.user,
                    },
                )?;
                if !self.no_follow {
                    loc = context
                        .with_current_dir(parent)?
                        .try_resolve_symlink(loc, &mut 0)?;
                }
                loc
            }
            Err(VfsError::InvalidInput) => {
                // root directory
                context.root_dir().clone()
            }
            Err(err) => return Err(err),
        };
        self._open(loc)
    }

    pub(crate) fn to_flags(&self) -> VfsResult<FileFlags> {
        Ok(match (self.read, self.write, self.append) {
            (true, false, false) => FileFlags::READ,
            (false, true, false) => FileFlags::WRITE,
            (true, true, false) => FileFlags::READ | FileFlags::WRITE,
            (false, _, true) => FileFlags::WRITE | FileFlags::APPEND,
            (true, _, true) => FileFlags::READ | FileFlags::WRITE | FileFlags::APPEND,
            (false, false, false) if self.path => FileFlags::empty(),
            (false, false, false) => return Err(VfsError::InvalidInput),
        } | if self.path {
            FileFlags::PATH
        } else {
            FileFlags::empty()
        })
    }

    pub(crate) fn is_valid(&self) -> bool {
        if !self.read && !self.write && !self.append {
            return true;
        }
        match (self.write, self.append) {
            (true, false) => {}
            (false, false) => {
                if self.truncate || self.create || self.create_new {
                    return false;
                }
            }
            (_, true) => {
                if self.truncate && !self.create_new {
                    return false;
                }
            }
        }
        true
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

const PAGE_SIZE: usize = 4096;
// Keep clean file pages while memory is plentiful.  Under pressure, reclaim
// old cache pages until the allocator has breathing room again.  This mirrors
// Linux's watermark-driven page-cache policy more closely than a fixed cache
// size: a small machine retains less, while an otherwise idle large machine
// can use most of its memory for useful cache.
const MIN_FREE_MEMORY_BYTES: usize = 192 * 1024 * 1024;
const MIN_FREE_TARGET_MEMORY_BYTES: usize = 256 * 1024 * 1024;
const FREE_MEMORY_LOW_DIVISOR: usize = 8;
const FREE_MEMORY_TARGET_DIVISOR: usize = 4;
const MAX_READAHEAD_BYTES: usize = 1024 * 1024;
const BUFFERED_READAHEAD_BYTES: usize = 128 * 1024;
const FOLIO_INDEX_SHARDS: usize = 64;
const FILE_CACHE_SHARDS: usize = 64;
const READAHEAD_WORKERS: usize = 8;
const MAX_FOLIO_ORDER: u8 = 4;
const MAX_FOLIO_PAGES: usize = 1 << MAX_FOLIO_ORDER;
const LOCAL_LARGE_FOLIO_CACHE_CAPACITY: usize = 8;
const LOCAL_PAGE_FRAME_CACHE_SHARDS: usize = 64;
const LARGE_FOLIO_RETRY_NANOS: u64 = 30 * 1_000_000_000;
static ZERO_PAGE: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
static FILE_CACHE_CLOCK: AtomicU64 = AtomicU64::new(1);
static LARGE_FOLIO_RETRY_AFTER_NS: [AtomicU64; MAX_FOLIO_ORDER as usize] =
    [const { AtomicU64::new(0) }; MAX_FOLIO_ORDER as usize];

#[derive(Default)]
struct SleepRwState {
    readers: usize,
    writer: bool,
    waiting_writers: usize,
}

/// A small sleeping read/write gate for operations that may block on storage.
///
/// `spin::RwLock` remains appropriate for the short folio-index critical
/// sections below, but must not be held across a block request.  This gate
/// gives page faults concurrent read access while truncate and invalidation
/// wait without burning a CPU.
struct SleepRwLock {
    state: Mutex<SleepRwState>,
    wait: WaitQueue,
}

impl SleepRwLock {
    const fn new() -> Self {
        Self {
            state: Mutex::new(SleepRwState {
                readers: 0,
                writer: false,
                waiting_writers: 0,
            }),
            wait: WaitQueue::new(),
        }
    }

    fn read(&self) -> SleepRwReadGuard<'_> {
        {
            let mut state = self.state.lock();
            if !state.writer && state.waiting_writers == 0 {
                state.readers += 1;
                return SleepRwReadGuard { lock: self };
            }
        }
        self.wait.wait_until(|| {
            let mut state = self.state.lock();
            if state.writer || state.waiting_writers != 0 {
                false
            } else {
                state.readers += 1;
                true
            }
        });
        SleepRwReadGuard { lock: self }
    }

    fn write(&self) -> SleepRwWriteGuard<'_> {
        let waiting = {
            let mut state = self.state.lock();
            state.waiting_writers += 1;
            if state.writer || state.readers != 0 {
                true
            } else {
                state.waiting_writers -= 1;
                state.writer = true;
                false
            }
        };
        if waiting {
            self.wait.wait_until(|| {
                let mut state = self.state.lock();
                if state.writer || state.readers != 0 {
                    false
                } else {
                    state.waiting_writers -= 1;
                    state.writer = true;
                    true
                }
            });
        }
        SleepRwWriteGuard { lock: self }
    }
}

struct SleepRwReadGuard<'a> {
    lock: &'a SleepRwLock,
}

impl Drop for SleepRwReadGuard<'_> {
    fn drop(&mut self) {
        let wake_writer = {
            let mut state = self.lock.state.lock();
            state.readers -= 1;
            state.readers == 0
        };
        if wake_writer {
            self.lock.wait.notify_all(false);
        }
    }
}

struct SleepRwWriteGuard<'a> {
    lock: &'a SleepRwLock,
}

impl Drop for SleepRwWriteGuard<'_> {
    fn drop(&mut self) {
        {
            let mut state = self.lock.state.lock();
            debug_assert!(state.writer);
            state.writer = false;
        }
        self.lock.wait.notify_all(false);
    }
}

struct LocalLargeFolioCache {
    frames: [[usize; LOCAL_LARGE_FOLIO_CACHE_CAPACITY]; MAX_FOLIO_ORDER as usize],
    lengths: [usize; MAX_FOLIO_ORDER as usize],
}

impl LocalLargeFolioCache {
    const fn new() -> Self {
        Self {
            frames: [[0; LOCAL_LARGE_FOLIO_CACHE_CAPACITY]; MAX_FOLIO_ORDER as usize],
            lengths: [0; MAX_FOLIO_ORDER as usize],
        }
    }

    fn pop(&mut self, order: u8) -> Option<VirtAddr> {
        let slot = order.checked_sub(1)? as usize;
        let len = &mut self.lengths[slot];
        if *len == 0 {
            return None;
        }
        *len -= 1;
        Some(self.frames[slot][*len].into())
    }

    fn push(&mut self, order: u8, frame: VirtAddr) -> bool {
        let Some(slot) = order.checked_sub(1).map(usize::from) else {
            return false;
        };
        let len = &mut self.lengths[slot];
        if *len == LOCAL_LARGE_FOLIO_CACHE_CAPACITY {
            return false;
        }
        self.frames[slot][*len] = frame.as_usize();
        *len += 1;
        true
    }
}

static LOCAL_LARGE_FOLIO_CACHES: [SpinNoIrq<LocalLargeFolioCache>; LOCAL_PAGE_FRAME_CACHE_SHARDS] =
    [const { SpinNoIrq::new(LocalLargeFolioCache::new()) }; LOCAL_PAGE_FRAME_CACHE_SHARDS];

fn local_large_folio_cache() -> &'static SpinNoIrq<LocalLargeFolioCache> {
    &LOCAL_LARGE_FOLIO_CACHES[axhal::percpu::this_cpu_id() % LOCAL_PAGE_FRAME_CACHE_SHARDS]
}

fn large_folio_backed_off(order: u8, now_ns: u64) -> bool {
    let slot = usize::from(order - 1);
    now_ns < LARGE_FOLIO_RETRY_AFTER_NS[slot].load(Ordering::Acquire)
}

fn back_off_large_folio(order: u8, now_ns: u64) {
    let slot = usize::from(order - 1);
    LARGE_FOLIO_RETRY_AFTER_NS[slot].store(
        now_ns.saturating_add(LARGE_FOLIO_RETRY_NANOS),
        Ordering::Release,
    );
}

fn alloc_page_frame() -> VfsResult<VirtAddr> {
    global_allocator()
        .alloc_page_cached(
            axhal::percpu::this_cpu_id(),
            axtask::active_cpu_count(),
            UsageKind::PageCache,
        )
        .map(VirtAddr::from)
        .inspect_err(|err| warn!("Failed to allocate page cache: {err:?}"))
        .map_err(|_| VfsError::NoMemory)
}

#[derive(Debug)]
struct FolioFrame {
    addr: VirtAddr,
    pages: usize,
}

impl Drop for FolioFrame {
    fn drop(&mut self) {
        if self.pages == 1 {
            global_allocator().dealloc_page_cached(
                axhal::percpu::this_cpu_id(),
                axtask::active_cpu_count(),
                self.addr.as_usize(),
                UsageKind::PageCache,
            );
            return;
        }
        if self.pages.is_power_of_two() {
            let order = self.pages.trailing_zeros() as u8;
            if (1..=MAX_FOLIO_ORDER).contains(&order)
                && local_large_folio_cache().lock().push(order, self.addr)
            {
                return;
            }
        }
        global_allocator().dealloc_pages(self.addr.as_usize(), self.pages, UsageKind::PageCache);
    }
}

fn alloc_folio_frame(order: u8) -> VfsResult<FolioFrame> {
    let pages = 1usize << order;
    let addr = if order == 0 {
        alloc_page_frame()?
    } else if let Some(frame) = local_large_folio_cache().lock().pop(order) {
        frame
    } else {
        let now_ns = axhal::time::monotonic_time_nanos();
        if large_folio_backed_off(order, now_ns) {
            return Err(VfsError::NoMemory);
        }
        let (start, frames) = match global_allocator().alloc_pages(
            pages * LOCAL_LARGE_FOLIO_CACHE_CAPACITY,
            pages * PAGE_SIZE,
            UsageKind::PageCache,
        ) {
            Ok(start) => (start, LOCAL_LARGE_FOLIO_CACHE_CAPACITY),
            Err(_) => match global_allocator().alloc_pages(
                pages,
                pages * PAGE_SIZE,
                UsageKind::PageCache,
            ) {
                Ok(start) => (start, 1),
                Err(_) => {
                    back_off_large_folio(order, now_ns);
                    return Err(VfsError::NoMemory);
                }
            },
        };
        LARGE_FOLIO_RETRY_AFTER_NS[usize::from(order - 1)].store(0, Ordering::Release);
        let frame = VirtAddr::from(start);
        let mut cache = local_large_folio_cache().lock();
        let mut first_uncached = frames;
        for index in 1..frames {
            if !cache.push(order, frame + index * pages * PAGE_SIZE) {
                first_uncached = index;
                break;
            }
        }
        drop(cache);
        for index in first_uncached..frames {
            global_allocator().dealloc_pages(
                (frame + index * pages * PAGE_SIZE).as_usize(),
                pages,
                UsageKind::PageCache,
            );
        }
        frame
    };
    Ok(FolioFrame { addr, pages })
}

#[derive(Debug)]
struct FolioState {
    loading: bool,
    uptodate: bool,
    error: Option<VfsError>,
    dirty: u16,
    writeback: u16,
    truncated: bool,
}

/// A physically contiguous, naturally aligned file-cache unit.
pub struct Folio {
    index: u32,
    order: u8,
    frame: FolioFrame,
    self_ref: Weak<Folio>,
    state: Mutex<FolioState>,
    page_locks: [Mutex<()>; MAX_FOLIO_PAGES],
    wait: WaitQueue,
    last_used: AtomicU64,
}

impl Folio {
    fn new_loading(index: u32, order: u8) -> VfsResult<Arc<Self>> {
        debug_assert!(order <= MAX_FOLIO_ORDER);
        debug_assert_eq!(index & ((1u32 << order) - 1), 0);
        let frame = alloc_folio_frame(order)?;
        Ok(Arc::new_cyclic(|this| Self {
            index,
            order,
            frame,
            self_ref: this.clone(),
            state: Mutex::new(FolioState {
                loading: true,
                uptodate: false,
                error: None,
                dirty: 0,
                writeback: 0,
                truncated: false,
            }),
            page_locks: [const { Mutex::new(()) }; MAX_FOLIO_PAGES],
            wait: WaitQueue::new(),
            last_used: AtomicU64::new(FILE_CACHE_CLOCK.fetch_add(1, Ordering::Relaxed)),
        }))
    }

    fn page_count(&self) -> usize {
        1usize << self.order
    }

    fn byte_len(&self) -> usize {
        self.page_count() * PAGE_SIZE
    }

    fn contains(&self, pn: u32) -> bool {
        pn >= self.index && pn < self.index + self.page_count() as u32
    }

    fn touch(&self) {
        self.last_used.store(
            FILE_CACHE_CLOCK.fetch_add(1, Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    fn page_offset(&self, pn: u32) -> usize {
        debug_assert!(self.contains(pn));
        (pn - self.index) as usize
    }

    fn paddr_at(&self, page: usize) -> PhysAddr {
        virt_to_phys(self.frame.addr + page * PAGE_SIZE)
    }

    unsafe fn data_mut_unchecked(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.frame.addr.as_mut_ptr(), self.byte_len()) }
    }

    fn complete_load(&self, result: VfsResult<()>) {
        let mut state = self.state.lock();
        state.loading = false;
        match result {
            Ok(()) if !state.truncated => {
                state.uptodate = true;
                state.error = None;
            }
            Ok(()) => {
                state.uptodate = false;
                state.error = Some(VfsError::Io);
            }
            Err(err) => {
                state.uptodate = false;
                state.error = Some(err);
            }
        }
        drop(state);
        self.wait.notify_all(false);
    }

    fn wait_uptodate(&self) -> VfsResult<()> {
        self.wait.wait_until(|| !self.state.lock().loading);
        let state = self.state.lock();
        if state.uptodate && !state.truncated {
            Ok(())
        } else {
            Err(state.error.unwrap_or(VfsError::Io))
        }
    }
}

impl Drop for Folio {
    fn drop(&mut self) {
        let state = self.state.lock();
        if state.dirty != 0 || state.writeback != 0 {
            warn!("dirty folio dropped without completing writeback");
        }
    }
}

/// A stable reference to one 4-KiB subpage of a folio.
#[derive(Clone)]
pub struct FolioPageRef {
    folio: Arc<Folio>,
    page: u8,
}

impl FolioPageRef {
    pub fn paddr(&self) -> PhysAddr {
        self.folio.paddr_at(self.page as usize)
    }

    /// Zero-based position of this page inside its folio.
    pub fn folio_page_offset(&self) -> usize {
        self.page as usize
    }

    /// Number of base pages owned by the same folio.
    pub fn folio_page_count(&self) -> usize {
        self.folio.page_count()
    }

    /// Returns another base-page reference from the same pinned folio.
    pub fn folio_page_ref(&self, page: usize) -> Option<Self> {
        (page < self.folio.page_count()).then(|| Self {
            folio: self.folio.clone(),
            page: page as u8,
        })
    }
}

pub struct FolioPage<'a> {
    folio: &'a Folio,
    page: usize,
    _page_guard: MutexGuard<'a, ()>,
}

impl FolioPage<'_> {
    pub fn paddr(&self) -> PhysAddr {
        self.folio.paddr_at(self.page)
    }

    pub fn page_ref(&self) -> FolioPageRef {
        FolioPageRef {
            folio: self
                .folio
                .self_ref
                .upgrade()
                .expect("folio page outlived its owning Arc"),
            page: self.page as u8,
        }
    }

    pub fn mark_dirty(&mut self) {
        self.folio.state.lock().dirty |= 1 << self.page;
    }

    pub fn data(&mut self) -> &mut [u8] {
        let offset = self.page * PAGE_SIZE;
        unsafe {
            core::slice::from_raw_parts_mut(
                (self.folio.frame.addr + offset).as_mut_ptr(),
                PAGE_SIZE,
            )
        }
    }
}

struct EvictListener {
    listener: Arc<dyn Fn(u32, &FolioPageRef) + Send + Sync>,
    link: LinkedListAtomicLink,
}

intrusive_adapter!(EvictListenerAdapter = Box<EvictListener>: EvictListener { link: LinkedListAtomicLink });

/// Per-inode file cache, equivalent to Linux's `address_space`.
///
/// The sharded index is the lookup fast path; `folio_heads` owns one canonical
/// reference per variable-order folio.
struct FileAddressSpace {
    folio_index: [RwLock<BTreeMap<u32, Arc<Folio>>>; FOLIO_INDEX_SHARDS],
    folio_heads: Mutex<BTreeMap<u32, Arc<Folio>>>,
    evict_listeners: Mutex<LinkedList<EvictListenerAdapter>>,
    /// Readers and writeback share this lock. Truncate and destructive
    /// invalidation take it exclusively and wait for in-flight folio I/O.
    invalidate_lock: SleepRwLock,
    /// Only one thread can append to the file at a time, while multiple writers
    /// are permitted.
    append_lock: SleepRwLock,
}

impl FileAddressSpace {
    pub fn new() -> Self {
        Self {
            folio_index: [const { RwLock::new(BTreeMap::new()) }; FOLIO_INDEX_SHARDS],
            folio_heads: Mutex::new(BTreeMap::new()),
            evict_listeners: Mutex::new(LinkedList::default()),
            invalidate_lock: SleepRwLock::new(),
            append_lock: SleepRwLock::new(),
        }
    }

    pub fn new_unbounded() -> Self {
        Self {
            folio_index: [const { RwLock::new(BTreeMap::new()) }; FOLIO_INDEX_SHARDS],
            folio_heads: Mutex::new(BTreeMap::new()),
            evict_listeners: Mutex::new(LinkedList::default()),
            invalidate_lock: SleepRwLock::new(),
            append_lock: SleepRwLock::new(),
        }
    }
}

type FileCacheKey = (usize, u64);

struct CachedFileRecord {
    key: FileCacheKey,
    location: Location,
    shared: Arc<FileAddressSpace>,
    in_memory: bool,
    last_used: AtomicU64,
}

impl CachedFileRecord {
    fn touch(&self) {
        self.last_used.store(
            FILE_CACHE_CLOCK.fetch_add(1, Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
}

static FILE_CACHES: [RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>>; FILE_CACHE_SHARDS] =
    [const { RwLock::new(BTreeMap::new()) }; FILE_CACHE_SHARDS];
static PAGE_CACHE_RECLAIM_LOCK: Mutex<()> = Mutex::new(());
const PRESSURE_RECLAIM_RETRY_PAGES: usize = 1024;
static NEXT_PRESSURE_RECLAIM_AT_PAGES: AtomicUsize = AtomicUsize::new(usize::MAX);

pub struct CachedFile {
    inner: Location,
    record: Arc<CachedFileRecord>,
}

impl Clone for CachedFile {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            record: self.record.clone(),
        }
    }
}

struct ReadaheadJob {
    cached: CachedFile,
    folio: Arc<Folio>,
}

static READAHEAD_QUEUE: Mutex<VecDeque<ReadaheadJob>> = Mutex::new(VecDeque::new());
static READAHEAD_WORK: WaitQueue = WaitQueue::new();
static READAHEAD_WORKERS_STARTED: AtomicBool = AtomicBool::new(false);

fn readahead_worker() -> ! {
    loop {
        READAHEAD_WORK.wait_until(|| !READAHEAD_QUEUE.lock().is_empty());
        let job = READAHEAD_QUEUE.lock().pop_front();
        if let Some(job) = job {
            job.cached.load_readahead_folio(job.folio);
        }
    }
}

fn start_readahead_workers() {
    if READAHEAD_WORKERS_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    for _ in 0..READAHEAD_WORKERS {
        axtask::spawn(|| readahead_worker());
    }
}

enum FileUserData {
    Weak(Weak<CachedFileRecord>),
}

impl FileUserData {
    pub fn get(&self) -> Option<Arc<CachedFileRecord>> {
        match self {
            FileUserData::Weak(weak) => weak.upgrade(),
        }
    }
}

impl CachedFile {
    fn file_cache_key(location: &Location) -> FileCacheKey {
        let fs_ptr: *const dyn axfs_ng_vfs::FilesystemOps = location.filesystem();
        (fs_ptr as *const () as usize, location.inode())
    }

    fn file_cache_shard(key: FileCacheKey) -> usize {
        ((key.0 >> 6) ^ key.1 as usize) % FILE_CACHE_SHARDS
    }

    fn cache_for_key(
        key: FileCacheKey,
    ) -> &'static RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>> {
        &FILE_CACHES[Self::file_cache_shard(key)]
    }

    fn all_records() -> Vec<Arc<CachedFileRecord>> {
        FILE_CACHES
            .iter()
            .flat_map(|cache| cache.read().values().cloned().collect::<Vec<_>>())
            .collect()
    }

    fn record_from_global_cache(location: &Location, in_memory: bool) -> Arc<CachedFileRecord> {
        let key = Self::file_cache_key(location);
        let mut caches = Self::cache_for_key(key).write();
        caches
            .entry(key)
            .or_insert_with(|| {
                let shared = if in_memory {
                    Arc::new(FileAddressSpace::new_unbounded())
                } else {
                    Arc::new(FileAddressSpace::new())
                };
                Arc::new(CachedFileRecord {
                    key,
                    location: location.clone(),
                    shared,
                    in_memory,
                    last_used: AtomicU64::new(FILE_CACHE_CLOCK.fetch_add(1, Ordering::Relaxed)),
                })
            })
            .clone()
    }

    pub fn get_or_create(location: Location) -> Self {
        let in_memory = location.filesystem().name() == "tmpfs";

        let mut guard = location.user_data();
        let record = if let Some(record) = guard.get::<FileUserData>().and_then(|it| it.get()) {
            record
        } else {
            let record = Self::record_from_global_cache(&location, in_memory);
            guard.insert(FileUserData::Weak(Arc::downgrade(&record)));
            record
        };
        drop(guard);
        record.touch();

        Self {
            inner: location,
            record,
        }
    }

    fn get_existing(location: &Location) -> Option<Self> {
        let key = Self::file_cache_key(location);
        Self::cache_for_key(key)
            .read()
            .get(&key)
            .cloned()
            .map(|record| {
                record.touch();
                Self {
                    inner: location.clone(),
                    record,
                }
            })
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.record, &other.record)
    }

    pub fn in_memory(&self) -> bool {
        self.record.in_memory
    }

    fn inner(&self) -> &Location {
        &self.inner
    }

    fn shared(&self) -> &FileAddressSpace {
        self.record.shared.as_ref()
    }

    fn folio_shard(pn: u32) -> usize {
        pn as usize % FOLIO_INDEX_SHARDS
    }

    fn lookup_folio(&self, pn: u32) -> Option<Arc<Folio>> {
        let folio = self.shared().folio_index[Self::folio_shard(pn)]
            .read()
            .get(&pn)
            .cloned();
        if let Some(folio) = folio.as_ref() {
            folio.touch();
        }
        folio
    }

    /// Publishes every base-page slot before starting I/O. `folio_heads`
    /// serializes the short multi-shard update, while cache hits never touch
    /// this lock.
    fn publish_folio(&self, folio: &Arc<Folio>) -> bool {
        let mut heads = self.shared().folio_heads.lock();
        let end = folio.index + folio.page_count() as u32;
        for pn in folio.index..end {
            if self.shared().folio_index[Self::folio_shard(pn)]
                .read()
                .contains_key(&pn)
            {
                return false;
            }
        }
        heads.insert(folio.index, folio.clone());
        for pn in folio.index..end {
            self.shared().folio_index[Self::folio_shard(pn)]
                .write()
                .insert(pn, folio.clone());
        }
        true
    }

    fn remove_folio(&self, folio: &Arc<Folio>) -> bool {
        let mut heads = self.shared().folio_heads.lock();
        if !heads
            .get(&folio.index)
            .is_some_and(|cached| Arc::ptr_eq(cached, folio))
        {
            return false;
        }
        heads.remove(&folio.index);
        let end = folio.index + folio.page_count() as u32;
        for pn in folio.index..end {
            let mut shard = self.shared().folio_index[Self::folio_shard(pn)].write();
            if shard
                .get(&pn)
                .is_some_and(|cached| Arc::ptr_eq(cached, folio))
            {
                shard.remove(&pn);
            }
        }
        true
    }

    fn choose_folio_order(start: u32, end: u32) -> u8 {
        let remaining = end.saturating_sub(start);
        for order in (0..=MAX_FOLIO_ORDER).rev() {
            let pages = 1u32 << order;
            if start.is_multiple_of(pages) && pages <= remaining {
                return order;
            }
        }
        0
    }

    fn get_or_publish_folio(&self, pn: u32, preferred_order: u8) -> VfsResult<(Arc<Folio>, bool)> {
        if let Some(folio) = self.lookup_folio(pn) {
            return Ok((folio, false));
        }

        let mut order = preferred_order.min(MAX_FOLIO_ORDER);
        loop {
            let pages = 1u32 << order;
            let index = pn & !(pages - 1);
            let folio = match Folio::new_loading(index, order) {
                Ok(folio) => folio,
                Err(VfsError::NoMemory) if order > 0 => {
                    order -= 1;
                    continue;
                }
                Err(err) => return Err(err),
            };
            if self.publish_folio(&folio) {
                return Ok((folio, true));
            }
            if let Some(folio) = self.lookup_folio(pn) {
                return Ok((folio, false));
            }
            if order == 0 {
                continue;
            }
            order -= 1;
        }
    }

    pub fn len(&self) -> VfsResult<u64> {
        self.inner().len()
    }

    pub fn metadata(&self) -> VfsResult<Metadata> {
        self.inner().metadata()
    }

    pub fn record_access(&self, atime: Duration, strict: bool) -> VfsResult<bool> {
        self.inner().record_access(atime, strict)
    }

    pub fn record_modification(&self, mtime: Duration) -> VfsResult<bool> {
        self.inner().record_modification(mtime)
    }

    fn grow_cached_len(&self, len: u64) -> VfsResult<u64> {
        self.inner().grow_cached_len(len, !self.in_memory())
    }

    fn set_cached_len(&self, len: u64, dirty: bool) -> VfsResult<()> {
        self.inner().set_cached_len(len, dirty)
    }

    fn commit_len(&self) -> VfsResult<u64> {
        let len = self.len()?;
        if !self.in_memory() {
            self.inner().flush_metadata_cache(true)?;
        }
        Ok(len)
    }

    fn commit_atime(&self) -> VfsResult<()> {
        self.inner().flush_metadata_cache(false)
    }

    fn read_fully_at(
        &self,
        file: &FileNode,
        mut buf: &mut [u8],
        mut offset: u64,
        file_len: u64,
    ) -> VfsResult<usize> {
        let mut read = 0;
        while !buf.is_empty() {
            #[cfg(feature = "ext4")]
            let count = if let Ok(inode) = self
                .inner()
                .entry()
                .downcast::<crate::fs::ext4::Inode>()
            {
                inode.read_cached_at(buf, offset, file_len)?
            } else {
                file.read_at(buf, offset)?
            };
            #[cfg(not(feature = "ext4"))]
            let count = file.read_at(buf, offset)?;
            if count == 0 {
                break;
            }
            read += count;
            offset += count as u64;
            buf = &mut buf[count..];
        }
        Ok(read)
    }

    fn write_fully_at(
        &self,
        file: &FileNode,
        mut buf: &[u8],
        mut offset: u64,
        file_len: u64,
    ) -> VfsResult<()> {
        while !buf.is_empty() {
            #[cfg(feature = "ext4")]
            let written = if let Ok(inode) = self
                .inner()
                .entry()
                .downcast::<crate::fs::ext4::Inode>()
            {
                inode.write_cached_at(buf, offset, file_len)?
            } else {
                file.write_at(buf, offset)?
            };
            #[cfg(not(feature = "ext4"))]
            let written = file.write_at(buf, offset)?;
            if written == 0 {
                return Err(VfsError::Io);
            }
            offset += written as u64;
            buf = &buf[written..];
        }
        Ok(())
    }

    fn from_record(record: &Arc<CachedFileRecord>) -> Self {
        Self {
            inner: record.location.clone(),
            record: record.clone(),
        }
    }

    pub fn add_evict_listener<F>(&self, listener: F) -> usize
    where
        F: Fn(u32, &FolioPageRef) + Send + Sync + 'static,
    {
        let pointer = Box::new(EvictListener {
            listener: Arc::new(listener),
            link: LinkedListAtomicLink::new(),
        });
        let handle = pointer.as_ref() as *const EvictListener as usize;
        self.shared().evict_listeners.lock().push_back(pointer);
        handle
    }

    pub unsafe fn remove_evict_listener(&self, handle: usize) {
        let mut guard = self.shared().evict_listeners.lock();
        let mut cursor = unsafe { guard.cursor_mut_from_ptr(handle as *const EvictListener) };
        cursor.remove();
    }

    fn notify_evict_listeners(&self, pn: u32, page: &FolioPageRef) {
        let listeners = self
            .shared()
            .evict_listeners
            .lock()
            .iter()
            .map(|listener| listener.listener.clone())
            .collect::<Vec<_>>();
        for listener in listeners {
            listener(pn, page);
        }
    }

    fn load_folio(
        &self,
        file: &FileNode,
        folio: &Arc<Folio>,
        zeroed: bool,
        file_len: u64,
    ) -> VfsResult<()> {
        let data = unsafe { folio.data_mut_unchecked() };
        let result = if zeroed || self.in_memory() {
            data.fill(0);
            Ok(())
        } else {
            self.read_fully_at(
                file,
                data,
                folio.index as u64 * PAGE_SIZE as u64,
                file_len,
            )
                .map(|read| data[read..].fill(0))
        };
        folio.complete_load(result);
        folio.wait_uptodate()
    }

    fn can_zero_fill_write_page(old_len: u64, page_start: u64, range: &Range<usize>) -> bool {
        if page_start >= old_len {
            return true;
        }
        let old_valid_end = (old_len - page_start).min(PAGE_SIZE as u64) as usize;
        range.start == 0 && range.end >= old_valid_end
    }

    fn with_folio_page<R>(
        &self,
        folio: &Arc<Folio>,
        pn: u32,
        f: impl FnOnce(&mut FolioPage<'_>) -> R,
    ) -> VfsResult<R> {
        folio.wait_uptodate()?;
        let page = folio.page_offset(pn);
        let page_guard = folio.page_locks[page].lock();
        let state = folio.state.lock();
        if !state.uptodate || state.truncated {
            return Err(state.error.unwrap_or(VfsError::Io));
        }
        drop(state);
        let mut page = FolioPage {
            folio,
            page,
            _page_guard: page_guard,
        };
        Ok(f(&mut page))
    }

    pub fn with_page<R>(&self, pn: u32, f: impl FnOnce(Option<&mut FolioPage<'_>>) -> R) -> R {
        let folio = {
            let _invalidate = self.shared().invalidate_lock.read();
            self.lookup_folio(pn)
        };
        let Some(folio) = folio else {
            return f(None);
        };

        // A readahead worker may own a newly published loading folio. Never
        // wait for that worker while holding invalidate_lock: a truncate can
        // queue as a writer, prevent the worker from acquiring its read side,
        // and then wait forever for this reader to leave.
        if folio.wait_uptodate().is_err() {
            return f(None);
        }

        let _invalidate = self.shared().invalidate_lock.read();
        let Some(current) = self.lookup_folio(pn) else {
            return f(None);
        };
        if !Arc::ptr_eq(&current, &folio) {
            return f(None);
        }
        let page_index = folio.page_offset(pn);
        let page_guard = folio.page_locks[page_index].lock();
        let state = folio.state.lock();
        if !state.uptodate || state.truncated {
            return f(None);
        }
        drop(state);
        let mut page = FolioPage {
            folio: &folio,
            page: page_index,
            _page_guard: page_guard,
        };
        f(Some(&mut page))
    }

    fn page_ref_if_cached(&self, pn: u32) -> Option<FolioPageRef> {
        let folio = self.lookup_folio(pn)?;
        let state = folio.state.lock();
        if state.loading || !state.uptodate || state.truncated {
            return None;
        }
        drop(state);
        Some(FolioPageRef {
            page: folio.page_offset(pn) as u8,
            folio,
        })
    }

    /// Runs a short mapping commit while truncate/invalidation is excluded.
    ///
    /// The caller performs all storage I/O before entering this function. It
    /// is intended only for publishing a PTE and its matching folio pin.
    pub fn with_valid_page_ref<R>(
        &self,
        pn: u32,
        page: &FolioPageRef,
        mark_dirty: bool,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        let _invalidate = self.shared().invalidate_lock.read();
        let cached = self.lookup_folio(pn)?;
        if !Arc::ptr_eq(&cached, &page.folio) || cached.page_offset(pn) != page.page as usize {
            return None;
        }
        let mut state = cached.state.lock();
        if state.loading || !state.uptodate || state.truncated {
            return None;
        }
        if mark_dirty && !self.in_memory() {
            state.dirty |= 1 << page.page;
        }
        drop(state);
        Some(commit())
    }

    /// Validates one pinned folio before publishing multiple sibling PTEs.
    fn with_valid_folio_ref<R>(
        &self,
        page: &FolioPageRef,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        let _invalidate = self.shared().invalidate_lock.read();
        let cached = self.lookup_folio(page.folio.index)?;
        if !Arc::ptr_eq(&cached, &page.folio) {
            return None;
        }
        let state = cached.state.lock();
        if state.loading || !state.uptodate || state.truncated {
            return None;
        }
        drop(state);
        Some(commit())
    }

    fn with_page_or_insert_impl<R>(
        &self,
        pn: u32,
        zeroed: bool,
        preferred_order: u8,
        f: impl FnOnce(&mut FolioPage<'_>, Option<(u32, FolioPageRef)>) -> VfsResult<R>,
    ) -> VfsResult<R> {
        // A speculative read may fail after publishing its loading folio.
        // Linux does not turn that stale readahead error directly into SIGBUS:
        // the foreground fault removes the failed cache entry and retries the
        // page once as demand I/O.
        let mut may_retry_failed_folio = true;
        loop {
            // Look up under the invalidation gate, but wait for an in-flight
            // loader only after dropping it. Once the folio becomes ready,
            // reacquire the gate and verify that the same folio is still
            // indexed before exposing its page.
            let cached = {
                let _invalidate = self.shared().invalidate_lock.read();
                self.lookup_folio(pn)
            };
            if let Some(folio) = cached {
                match folio.wait_uptodate() {
                    Ok(()) => {
                        let _invalidate = self.shared().invalidate_lock.read();
                        let current = self.lookup_folio(pn);
                        if current
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &folio))
                        {
                            return self.with_folio_page(&folio, pn, |page| f(page, None))?;
                        }
                        continue;
                    }
                    Err(_err) if may_retry_failed_folio => {
                        let _invalidate = self.shared().invalidate_lock.read();
                        if self
                            .lookup_folio(pn)
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &folio))
                        {
                            self.remove_folio(&folio);
                        }
                        may_retry_failed_folio = false;
                    }
                    Err(err) => return Err(err),
                }
            }

            Self::reclaim_page_cache_if_needed(self.record.key);
            let _invalidate = self.shared().invalidate_lock.read();
            let file = self.inner().entry().as_file()?;
            let file_len = self.len()?;
            let (folio, loader) = self.get_or_publish_folio(pn, preferred_order)?;
            if !loader {
                // Another task published the folio after the lock-free lookup.
                // Drop invalidate_lock before waiting for that task at the top
                // of the loop.
                continue;
            }
            let load_result = self.load_folio(file, &folio, zeroed, file_len);
            if let Err(err) = load_result {
                self.remove_folio(&folio);
                if may_retry_failed_folio {
                    may_retry_failed_folio = false;
                    continue;
                }
                return Err(err);
            }
            return self.with_folio_page(&folio, pn, |page| f(page, None))?;
        }
    }

    pub fn with_page_or_insert<R>(
        &self,
        pn: u32,
        f: impl FnOnce(&mut FolioPage<'_>, Option<(u32, FolioPageRef)>) -> VfsResult<R>,
    ) -> VfsResult<R> {
        self.with_page_or_insert_impl(pn, false, 0, f)
    }

    /// Demand-loads the naturally aligned folio containing a file-mapping
    /// fault. Unlike advisory readahead, the faulting task owns this load and
    /// waits for the whole folio, so neighboring faults coalesce behind one
    /// request without putting speculative I/O ahead of the demanded page.
    pub fn fault_page_ref(&self, pn: u32) -> VfsResult<FolioPageRef> {
        let file_pages = self.len()?.div_ceil(PAGE_SIZE as u64).max(1);
        let order = (u64::BITS - 1 - file_pages.leading_zeros()) as u8;
        self.with_page_or_insert_impl(pn, false, order.min(MAX_FOLIO_ORDER), |page, _| {
            Ok(page.page_ref())
        })
    }

    fn with_pages<T>(
        &self,
        range: Range<u64>,
        page_initial: impl FnOnce(&FileNode) -> VfsResult<T>,
        mut page_each: impl FnMut(T, &mut FolioPage<'_>, Range<usize>) -> VfsResult<T>,
    ) -> VfsResult<T> {
        let file = self.inner().entry().as_file()?;
        let mut initial = Some(page_initial(file)?);
        let start_page = (range.start / PAGE_SIZE as u64) as u32;
        let end_page = range.end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (range.start % PAGE_SIZE as u64) as usize;
        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let page_range = page_offset..(range.end - page_start).min(PAGE_SIZE as u64) as usize;

            let current = initial.take().unwrap();
            // A foreground buffered read must wait only for the range the
            // caller actually requested. Larger speculative windows belong to
            // `readahead()`: synchronously turning every 4-KiB header read
            // into 64 KiB of I/O amplifies Cargo's many short file reads.
            let order = Self::choose_folio_order(pn, end_page);
            initial = Some(self.with_page_or_insert_impl(pn, false, order, |page, _| {
                page_each(current, page, page_range)
            })?);
            page_offset = 0;
        }

        Ok(initial.unwrap())
    }

    pub fn read_at(&self, mut dst: impl Write + IoBufMut, offset: u64) -> VfsResult<usize> {
        let len = self.len()?;
        let end = offset.saturating_add(dst.remaining_mut() as u64).min(len);
        if end <= offset {
            return Ok(0);
        }
        if self.in_memory() {
            return self.read_at_in_memory(dst, offset, end);
        }
        self.with_pages(
            offset..end,
            |_| Ok(0),
            |read, page, range| {
                let len = range.end - range.start;
                dst.write_all(&page.data()[range.start..range.end])?;
                Ok(read + len)
            },
        )
    }

    pub fn readahead(&self, offset: u64, count: usize) -> VfsResult<()> {
        if count == 0 || self.in_memory() {
            return Ok(());
        }

        let (low, _) = Self::memory_watermarks();
        if global_allocator().available_pages() < low {
            // Speculative I/O must yield its memory budget to demand faults.
            return Ok(());
        }

        let len = self.len()?;
        if offset >= len {
            return Ok(());
        }
        let count = count.min(MAX_READAHEAD_BYTES);
        let end = offset.saturating_add(count as u64).min(len);
        let mut start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;
        Self::reclaim_page_cache_if_needed(self.record.key);
        let _invalidate = self.shared().invalidate_lock.read();
        let mut loaders = Vec::new();
        let mut prepare_error = None;
        while start_page < end_page {
            if let Some(folio) = self.lookup_folio(start_page) {
                // An existing loading folio already represents an in-flight
                // read. Readahead is advisory and must not wait for it.
                start_page = (folio.index + folio.page_count() as u32).min(end_page);
                continue;
            }

            let order = Self::choose_folio_order(start_page, end_page);
            match self.get_or_publish_folio(start_page, order) {
                Ok((folio, true)) => {
                    start_page = (folio.index + folio.page_count() as u32).min(end_page);
                    loaders.push(folio);
                }
                Ok((folio, false)) => {
                    start_page = (folio.index + folio.page_count() as u32).min(end_page);
                }
                Err(err) => {
                    prepare_error = Some(err);
                    break;
                }
            }
        }
        drop(_invalidate);

        if !loaders.is_empty() {
            start_readahead_workers();
            let count = loaders.len();
            let mut queue = READAHEAD_QUEUE.lock();
            queue.extend(loaders.into_iter().map(|folio| ReadaheadJob {
                cached: self.clone(),
                folio,
            }));
            drop(queue);
            READAHEAD_WORK.notify_many(count.min(READAHEAD_WORKERS), false);
        }
        prepare_error.map_or(Ok(()), Err)
    }

    fn load_readahead_folio(&self, folio: Arc<Folio>) {
        let _invalidate = self.shared().invalidate_lock.read();
        let Ok(file) = self.inner().entry().as_file() else {
            folio.complete_load(Err(VfsError::Io));
            self.remove_folio(&folio);
            return;
        };
        let Ok(file_len) = self.len() else {
            folio.complete_load(Err(VfsError::Io));
            self.remove_folio(&folio);
            return;
        };

        let loading = {
            let state = folio.state.lock();
            state.loading && !state.truncated
        };
        let folio_start = u64::from(folio.index) * PAGE_SIZE as u64;
        if !loading {
            return;
        }
        if folio_start >= file_len {
            folio.complete_load(Err(VfsError::Io));
            self.remove_folio(&folio);
            return;
        }
        if self
            .load_folio(file, &folio, false, file_len)
            .is_err()
        {
            self.remove_folio(&folio);
        }
    }

    fn read_at_in_memory(
        &self,
        mut dst: impl Write + IoBufMut,
        offset: u64,
        end: u64,
    ) -> VfsResult<usize> {
        let _invalidate = self.shared().invalidate_lock.read();
        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (offset % PAGE_SIZE as u64) as usize;
        let mut read = 0;

        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let range_start = page_offset;
            let range_end = (end - page_start).min(PAGE_SIZE as u64) as usize;
            let len = range_end - range_start;

            if let Some(folio) = self.lookup_folio(pn) {
                self.with_folio_page(&folio, pn, |page| {
                    dst.write_all(&page.data()[range_start..range_end])
                })??;
            } else {
                dst.write_all(&ZERO_PAGE[range_start..range_end])?;
            }
            read += len;
            page_offset = 0;
        }

        Ok(read)
    }

    fn write_at_locked(&self, mut buf: impl Read + IoBuf, offset: u64) -> VfsResult<usize> {
        let end = offset
            .checked_add(buf.remaining() as u64)
            .ok_or(VfsError::InvalidInput)?;
        if end <= offset {
            return Ok(0);
        }
        if self.in_memory() {
            return self.write_at_in_memory(buf, offset, end);
        }

        let old_len = self.len()?;
        if end > old_len {
            self.grow_cached_len(end)?;
        }

        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (offset % PAGE_SIZE as u64) as usize;
        let mut written = 0;

        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let range = page_offset..(end - page_start).min(PAGE_SIZE as u64) as usize;
            let len = range.end - range.start;

            let zeroed = Self::can_zero_fill_write_page(old_len, page_start, &range);
            let order = Self::choose_folio_order(pn, end_page);
            self.with_page_or_insert_impl(pn, zeroed, order, |page, _| {
                buf.read_exact(&mut page.data()[range.start..range.end])?;
                page.mark_dirty();
                Ok(())
            })?;
            written += len;
            page_offset = 0;
        }

        Ok(written)
    }

    fn write_at_in_memory(
        &self,
        mut buf: impl Read + IoBuf,
        offset: u64,
        end: u64,
    ) -> VfsResult<usize> {
        let file = self.inner().entry().as_file()?;
        if end > self.len()? {
            file.set_len(end)?;
            self.set_cached_len(end, false)?;
        }

        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (offset % PAGE_SIZE as u64) as usize;
        let mut written = 0;

        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let range_start = page_offset;
            let range_end = (end - page_start).min(PAGE_SIZE as u64) as usize;
            let len = range_end - range_start;

            let mut data = Vec::new();
            data.resize(len, 0);
            buf.read_exact(&mut data)?;
            if data.iter().any(|byte| *byte != 0) || self.lookup_folio(pn).is_some() {
                self.with_page_or_insert_impl(pn, true, 0, |page, _| {
                    page.data()[range_start..range_end].copy_from_slice(&data);
                    Ok(())
                })?;
            }
            written += len;
            page_offset = 0;
        }

        Ok(written)
    }

    pub fn write_at(&self, buf: impl Read + IoBuf, offset: u64) -> VfsResult<usize> {
        let _guard = self.shared().append_lock.read();
        self.write_at_locked(buf, offset)
    }

    pub fn append(&self, buf: impl Read + IoBuf) -> VfsResult<(usize, u64)> {
        let _guard = self.shared().append_lock.write();
        let len = self.len()?;
        self.write_at_locked(buf, len)
            .map(|written| (written, len + written as u64))
    }

    pub fn set_len(&self, len: u64) -> VfsResult<()> {
        let _append = self.shared().append_lock.write();
        let old_len = self.len()?;
        if old_len == len {
            return Ok(());
        }

        // Dirty data below a shrinking EOF must reach storage before ext4
        // releases the corresponding blocks.
        if len < old_len && !self.in_memory() {
            self.writeback_folios_in_range(0..len)?;
        }

        let invalidate = self.shared().invalidate_lock.write();
        let file = self.inner().entry().as_file()?;
        file.set_len(len)?;
        self.set_cached_len(len, false)?;

        let mut removed = Vec::new();
        let mut invalid_pages = Vec::new();
        if old_len < len {
            for folio in self.folios_snapshot() {
                let start = folio.index as u64 * PAGE_SIZE as u64;
                let end = start + folio.byte_len() as u64;
                let zero_start = old_len.max(start);
                let zero_end = len.min(end);
                if zero_start >= zero_end {
                    continue;
                }
                let state = folio.state.lock();
                if state.uptodate && !state.truncated {
                    let data = unsafe { folio.data_mut_unchecked() };
                    data[(zero_start - start) as usize..(zero_end - start) as usize].fill(0);
                }
                drop(state);
            }
        } else {
            let first_invalid_page = len.div_ceil(PAGE_SIZE as u64) as u32;
            for folio in self.folios_snapshot() {
                let start = folio.index as u64 * PAGE_SIZE as u64;
                let end = start + folio.byte_len() as u64;
                if start >= len {
                    if self.remove_folio(&folio) {
                        let mut state = folio.state.lock();
                        state.truncated = true;
                        state.uptodate = false;
                        state.dirty = 0;
                        state.error = Some(VfsError::Io);
                        drop(state);
                        removed.push(folio);
                        continue;
                    }
                } else if end > len {
                    let mut state = folio.state.lock();
                    let data = unsafe { folio.data_mut_unchecked() };
                    data[(len - start) as usize..].fill(0);
                    let valid_pages = (len - start).div_ceil(PAGE_SIZE as u64) as usize;
                    let valid_mask = if valid_pages >= u16::BITS as usize {
                        u16::MAX
                    } else {
                        (1u16 << valid_pages) - 1
                    };
                    state.dirty &= valid_mask;
                }
                for pn in
                    first_invalid_page.max(folio.index)..folio.index + folio.page_count() as u32
                {
                    let page = FolioPageRef {
                        folio: folio.clone(),
                        page: folio.page_offset(pn) as u8,
                    };
                    invalid_pages.push((pn, page));
                }
            }
        }
        drop(invalidate);
        for (pn, page) in invalid_pages {
            self.notify_evict_listeners(pn, &page);
        }
        for folio in removed {
            self.notify_folio_evict(&folio);
        }
        Ok(())
    }

    fn folios_snapshot(&self) -> Vec<Arc<Folio>> {
        self.shared().folio_heads.lock().values().cloned().collect()
    }

    fn notify_folio_evict(&self, folio: &Arc<Folio>) {
        for page in 0..folio.page_count() {
            let reference = FolioPageRef {
                folio: folio.clone(),
                page: page as u8,
            };
            self.notify_evict_listeners(folio.index + page as u32, &reference);
        }
    }

    fn folio_overlaps(folio: &Folio, range: &Range<u64>) -> bool {
        let start = folio.index as u64 * PAGE_SIZE as u64;
        let end = start + folio.byte_len() as u64;
        start < range.end && end > range.start
    }

    fn writeback_folio(
        &self,
        file: &FileNode,
        file_len: u64,
        folio: &Arc<Folio>,
    ) -> VfsResult<bool> {
        folio.wait_uptodate()?;
        folio.wait.wait_until(|| folio.state.lock().writeback == 0);

        let folio_start = folio.index as u64 * PAGE_SIZE as u64;
        let valid_bytes = file_len
            .saturating_sub(folio_start)
            .min(folio.byte_len() as u64) as usize;
        let index_refs = folio.page_count() + 2;
        let externally_pinned = Arc::strong_count(folio) > index_refs;
        let dirty = {
            let mut state = folio.state.lock();
            if state.truncated {
                return Ok(false);
            }
            if valid_bytes == 0 {
                state.dirty = 0;
                return Ok(false);
            }
            let valid_pages = valid_bytes.div_ceil(PAGE_SIZE);
            let valid_mask = if valid_pages >= u16::BITS as usize {
                u16::MAX
            } else {
                (1u16 << valid_pages) - 1
            };
            state.dirty &= valid_mask;
            let dirty = state.dirty;
            if dirty == 0 {
                return Ok(false);
            }
            state.dirty &= !dirty;
            state.writeback |= dirty;
            dirty
        };
        let mut data = Vec::new();
        data.resize(valid_bytes, 0);
        for page in 0..folio.page_count() {
            if dirty & (1 << page) == 0 {
                continue;
            }
            let _page_guard = folio.page_locks[page].lock();
            let start = page * PAGE_SIZE;
            let end = (start + PAGE_SIZE).min(valid_bytes);
            if start < end {
                data[start..end]
                    .copy_from_slice(&unsafe { folio.data_mut_unchecked() }[start..end]);
            }
        }

        let mut result = Ok(());
        let mut page = 0;
        while page < folio.page_count() {
            if dirty & (1 << page) == 0 {
                page += 1;
                continue;
            }
            let run_start = page;
            while page < folio.page_count() && dirty & (1 << page) != 0 {
                page += 1;
            }
            let byte_start = run_start * PAGE_SIZE;
            let byte_end = (page * PAGE_SIZE).min(data.len());
            if byte_start < byte_end {
                result = self.write_fully_at(
                    file,
                    &data[byte_start..byte_end],
                    folio_start + byte_start as u64,
                    file_len,
                );
                if result.is_err() {
                    break;
                }
            }
        }

        let mut state = folio.state.lock();
        state.writeback &= !dirty;
        if result.is_err() || externally_pinned || Arc::strong_count(folio) > index_refs {
            // A writable mmap can modify the frame after the bounce copy was
            // taken without taking FolioState. Keep pinned pages dirty until
            // their PTE is removed, preventing a later modification from
            // being mistaken for already-written data.
            state.dirty |= dirty;
        }
        drop(state);
        folio.wait.notify_all(false);
        result.map(|_| true)
    }

    fn writeback_folios_in_range(&self, range: Range<u64>) -> VfsResult<()> {
        if self.in_memory() || range.start >= range.end {
            return Ok(());
        }

        let file = self.inner().entry().as_file()?;
        let file_len = self.len()?;
        let folios = self.folios_snapshot();
        let mut wrote = false;
        for folio in folios {
            if Self::folio_overlaps(&folio, &range) {
                // A speculative folio may still be owned by a readahead
                // worker. Wait before entering the invalidation gate so a
                // queued truncate cannot block that worker behind us.
                folio.wait_uptodate()?;
                let _invalidate = self.shared().invalidate_lock.read();
                wrote |= self.writeback_folio(file, file_len, &folio)?;
            }
        }
        if wrote {
            self.commit_len()?;
        }
        Ok(())
    }

    fn writeback_dirty_pages_in_range(&self, range: Range<u64>) -> VfsResult<()> {
        self.writeback_folios_in_range(range)
    }

    fn invalidate_clean_pages_in_range(&self, range: Range<u64>) {
        if self.in_memory() || range.start >= range.end {
            return;
        }
        let invalidate = self.shared().invalidate_lock.write();
        let mut removed = Vec::new();
        for folio in self.folios_snapshot() {
            if !Self::folio_overlaps(&folio, &range) {
                continue;
            }
            let clean = {
                let state = folio.state.lock();
                !state.loading && state.dirty == 0 && state.writeback == 0
            };
            if clean && self.remove_folio(&folio) {
                removed.push(folio);
            }
        }
        drop(invalidate);
        for folio in removed {
            self.notify_folio_evict(&folio);
        }
    }

    fn discard_cached_pages(&self) {
        let invalidate = self.shared().invalidate_lock.write();
        let mut removed = Vec::new();
        for folio in self.folios_snapshot() {
            if self.remove_folio(&folio) {
                {
                    let mut state = folio.state.lock();
                    state.loading = false;
                    state.uptodate = false;
                    state.error = Some(VfsError::Io);
                    state.dirty = 0;
                    state.writeback = 0;
                    state.truncated = true;
                }
                folio.wait.notify_all(false);
                removed.push(folio);
            }
        }
        drop(invalidate);

        for folio in removed {
            self.notify_folio_evict(&folio);
        }
    }

    fn memory_watermarks() -> (usize, usize) {
        let allocator = global_allocator();
        let total = allocator.used_pages() + allocator.available_pages();
        let minimum = MIN_FREE_MEMORY_BYTES / PAGE_SIZE;
        let low = (total / FREE_MEMORY_LOW_DIVISOR).max(minimum).min(total);
        let target = (total / FREE_MEMORY_TARGET_DIVISOR)
            .max(MIN_FREE_TARGET_MEMORY_BYTES / PAGE_SIZE)
            .max(low)
            .min(total);
        (low, target)
    }

    /// Reclaim least-recently-used clean folios. Mapped folios remain pinned
    /// until the owning PTE is removed.
    ///
    /// Memory-pressure reclaim must not start writeback. The block layer and
    /// ext4 path can themselves need page-cache or heap memory, so trying to
    /// write dirty folios after the allocator reaches its low watermark turns
    /// pressure into a recursive allocation failure. Normal fsync/close paths
    /// remain responsible for dirty writeback.
    fn reclaim_pages(&self, limit: usize) -> usize {
        if self.in_memory() || limit == 0 {
            return 0;
        }

        let mut folios = self.folios_snapshot();
        folios.sort_unstable_by_key(|folio| folio.last_used.load(Ordering::Relaxed));

        let mut reclaimed = 0;
        for folio in folios {
            if reclaimed >= limit {
                break;
            }

            let invalidate = self.shared().invalidate_lock.write();
            let clean = {
                let state = folio.state.lock();
                !state.loading && state.dirty == 0 && state.writeback == 0 && !state.truncated
            };
            let index_refs = folio.page_count() + 2;
            if clean && Arc::strong_count(&folio) <= index_refs && self.remove_folio(&folio) {
                reclaimed += folio.page_count();
                drop(invalidate);
                self.notify_folio_evict(&folio);
            } else {
                drop(invalidate);
            }
        }
        reclaimed
    }

    fn reclaim_page_cache_if_needed(retain_key: FileCacheKey) {
        let allocator = global_allocator();
        let (low, target) = Self::memory_watermarks();
        if allocator.available_pages() >= low {
            return;
        }

        // A demand file fault must wait for an in-flight reclaim pass. If it
        // races ahead while free memory is below the low watermark, its folio
        // allocation can fail and userspace receives SIGBUS even though the
        // other task is about to release clean pages.
        let _reclaim_guard = PAGE_CACHE_RECLAIM_LOCK.lock();
        if allocator.available_pages() >= low {
            return;
        }

        Self::reclaim_page_caches_to(target, Some(retain_key));
    }

    fn reclaim_page_caches_to(target: usize, retain_key: Option<FileCacheKey>) {
        let allocator = global_allocator();

        let mut records = Self::all_records()
            .into_iter()
            .filter(|record| !record.in_memory)
            .collect::<Vec<_>>();
        records.sort_unstable_by_key(|record| {
            (
                Some(record.key) == retain_key,
                record.last_used.load(Ordering::Relaxed),
            )
        });

        for record in records {
            let available = allocator.available_pages();
            if available >= target {
                break;
            }
            let wanted = target - available;
            let cached = Self::from_record(&record);
            cached.reclaim_pages(wanted);
        }
    }

    fn writeback_dirty_pages_in_cache(&self) -> VfsResult<()> {
        if self.in_memory() {
            return Ok(());
        }
        self.writeback_folios_in_range(0..u64::MAX)
    }

    fn drop_cached_pages(&self) -> VfsResult<()> {
        if self.in_memory() {
            return Ok(());
        }
        self.writeback_dirty_pages_in_cache()?;
        self.invalidate_clean_pages_in_range(0..u64::MAX);
        self.commit_atime()
    }

    pub fn sync(&self, data_only: bool) -> VfsResult<()> {
        if self.in_memory() {
            return Ok(());
        }
        self.writeback_dirty_pages_in_cache()?;
        if !data_only {
            self.commit_atime()?;
        }
        let file = self.inner().entry().as_file()?;
        file.sync(data_only)?;
        Ok(())
    }

    pub fn location(&self) -> &Location {
        self.inner()
    }
}

impl Drop for CachedFile {
    fn drop(&mut self) {
        if self.in_memory() || Arc::strong_count(&self.record) != 2 {
            return;
        }
        self.record.touch();
        Self::reclaim_page_cache_if_needed(self.record.key);
    }
}

/// Reclaims clean file-cache folios before an anonymous user-page allocation
/// consumes the final free pages.
///
/// File reads already enter watermark reclaim while growing the page cache.
/// Anonymous COW/brk faults do not grow that cache, so without this hook a
/// compiler can consume the remaining RAM while old source and object pages
/// are still reclaimable. Serialize pressure reclaim and restore the normal
/// target watermark before the caller retries allocation.
pub fn reclaim_global_file_cache_for_memory_pressure() {
    let allocator = global_allocator();
    let (low, target) = CachedFile::memory_watermarks();
    let available = allocator.available_pages();
    if available >= low {
        NEXT_PRESSURE_RECLAIM_AT_PAGES.store(usize::MAX, Ordering::Release);
        return;
    }
    if available > NEXT_PRESSURE_RECLAIM_AT_PAGES.load(Ordering::Acquire) {
        return;
    }

    let _reclaim_guard = PAGE_CACHE_RECLAIM_LOCK.lock();
    let available = allocator.available_pages();
    if available >= low {
        NEXT_PRESSURE_RECLAIM_AT_PAGES.store(usize::MAX, Ordering::Release);
        return;
    }
    if available > NEXT_PRESSURE_RECLAIM_AT_PAGES.load(Ordering::Acquire) {
        return;
    }
    CachedFile::reclaim_page_caches_to(target, None);
    let available = allocator.available_pages();
    let next = if available >= low {
        usize::MAX
    } else {
        available.saturating_sub(PRESSURE_RECLAIM_RETRY_PAGES)
    };
    NEXT_PRESSURE_RECLAIM_AT_PAGES.store(next, Ordering::Release);
}

pub fn sync_global_file_caches() -> VfsResult<()> {
    let records = CachedFile::all_records();
    for record in &records {
        if record.in_memory {
            continue;
        }
        let cached = CachedFile::from_record(record);
        cached.writeback_dirty_pages_in_cache()?;
        cached.commit_atime()?;
    }
    flush_inode_metadata_caches(false)?;

    // An inode sync in ext4 flushes the whole shared filesystem. Running it
    // once per cached file turns a single sync(2) into thousands of identical
    // full-cache flushes after a build, so deduplicate by filesystem identity.
    let mut flushed = BTreeSet::new();
    for record in &records {
        if record.in_memory {
            continue;
        }
        let filesystem = record.location.filesystem();
        let pointer = filesystem as *const dyn axfs_ng_vfs::FilesystemOps as *const () as usize;
        if flushed.insert(pointer) {
            filesystem.flush()?;
        }
    }
    Ok(())
}

pub fn drop_global_file_caches() -> VfsResult<()> {
    let records = CachedFile::all_records();
    for record in records {
        if record.in_memory {
            continue;
        }
        CachedFile::from_record(&record).drop_cached_pages()?;
    }
    Ok(())
}

pub fn drop_file_caches_by_device(device: u64) -> VfsResult<()> {
    let records = FILE_CACHES
        .iter()
        .flat_map(|cache| {
            cache
                .read()
                .iter()
                .filter(|(_, record)| record.location.mountpoint().device() == device)
                .map(|(key, record)| (*key, record.clone()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    for (_, record) in &records {
        CachedFile::from_record(record).drop_cached_pages()?;
    }

    for (key, record) in records {
        let mut caches = CachedFile::cache_for_key(key).write();
        if caches
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &record))
        {
            caches.remove(&key);
            record.location.user_data().remove::<FileUserData>();
            record.location.invalidate_metadata_cache();
        }
    }
    Ok(())
}

pub fn invalidate_file_cache(location: &Location) -> VfsResult<()> {
    let key = CachedFile::file_cache_key(location);
    let cache = CachedFile::cache_for_key(key);
    let Some(record) = cache.read().get(&key).cloned() else {
        location.invalidate_metadata_cache();
        return Ok(());
    };
    debug_assert_eq!(record.key, key);
    if record.in_memory {
        cache.write().remove(&key);
        location.user_data().remove::<FileUserData>();
        location.invalidate_metadata_cache();
        return Ok(());
    }
    CachedFile::from_record(&record).drop_cached_pages()?;
    cache.write().remove(&key);
    location.user_data().remove::<FileUserData>();
    location.invalidate_metadata_cache();
    Ok(())
}

pub fn invalidate_removed_file_cache(location: &Location) -> VfsResult<()> {
    let key = CachedFile::file_cache_key(location);
    let cache = CachedFile::cache_for_key(key);
    let Some(record) = cache.read().get(&key).cloned() else {
        // 这里只是 namespace 修改前的准备阶段；失败时必须仍可继续使用原记录。
        location.invalidate_metadata_cache();
        return Ok(());
    };

    debug_assert_eq!(record.key, key);
    if !record.in_memory {
        CachedFile::from_record(&record).drop_cached_pages()?;
    }
    location.invalidate_metadata_cache();
    Ok(())
}

/// 在 unlink 或 rename 替换成功后提交缓存记录退役。
pub fn finalize_removed_file_cache(location: &Location) {
    let key = CachedFile::file_cache_key(location);
    let cache = CachedFile::cache_for_key(key);
    let Some(record) = cache.read().get(&key).cloned() else {
        location.discard_metadata_cache();
        return;
    };

    debug_assert_eq!(record.key, key);
    // tmpfs 的页就是文件内容；只有没有打开文件或 mmap 持有记录时才能丢弃。
    if record.in_memory && Arc::strong_count(&record) == 2 {
        CachedFile::from_record(&record).discard_cached_pages();
    }
    cache.write().remove(&key);
    location.user_data().remove::<FileUserData>();
    location.discard_metadata_cache();
}

pub fn cached_file_len(location: &Location) -> Option<u64> {
    location.cached_metadata().map(|metadata| metadata.size)
}

pub fn set_cached_len_clean(location: &Location, len: u64) {
    location.set_cached_len_if_present(len);
}

pub fn update_cached_metadata(location: &Location, mut update: MetadataUpdate) -> VfsResult<()> {
    if update.ctime.is_none()
        && (update.mode.is_some() || update.owner.is_some() || update.mtime.is_some())
    {
        update.ctime = Some(axhal::time::realtime_time());
    }
    location.update_metadata(update)
}

pub fn cached_metadata(location: &Location) -> VfsResult<Metadata> {
    location.metadata()
}

pub fn metadata_with_cached_len(location: &Location) -> VfsResult<Metadata> {
    cached_metadata(location)
}

/// Low-level interface for file operations.
#[derive(Clone)]
pub enum FileBackend {
    Cached(CachedFile),
    Direct(Location),
}

impl FileBackend {
    pub(crate) fn new_direct(location: Location) -> Self {
        Self::Direct(location)
    }

    pub(crate) fn new_cached(location: Location) -> Self {
        Self::Cached(CachedFile::get_or_create(location))
    }

    pub fn read_at(&self, mut dst: impl Write + IoBufMut, mut offset: u64) -> VfsResult<usize> {
        match self {
            Self::Cached(cached) => {
                let requested = dst.remaining_mut();
                let read = cached.read_at(dst, offset)?;
                // Buffered consumers such as rustc normally continue after a
                // full-page read. Submit the next window asynchronously; tiny
                // ELF/header probes stay demand-only, and the shared folio
                // index coalesces overlap between processes.
                if read >= PAGE_SIZE && requested >= PAGE_SIZE {
                    let _ = cached
                        .readahead(offset.saturating_add(read as u64), BUFFERED_READAHEAD_BYTES);
                }
                Ok(read)
            }
            Self::Direct(loc) => {
                let start = offset;
                let len = dst.remaining_mut();
                if len > 0
                    && let Some(cached) = CachedFile::get_existing(loc)
                {
                    cached
                        .writeback_dirty_pages_in_range(start..start.saturating_add(len as u64))?;
                }
                dst.read_from(&mut axio::read_fn(|buf| {
                    loc.entry().as_file()?.read_at(buf, offset).inspect(|read| {
                        offset += *read as u64;
                    })
                }))
            }
        }
    }

    pub fn write_at(&self, mut src: impl Read + IoBuf, mut offset: u64) -> VfsResult<usize> {
        match self {
            Self::Cached(cached) => cached.write_at(src, offset),
            Self::Direct(loc) => {
                let start = offset;
                let len = src.remaining();
                let cached = CachedFile::get_existing(loc);
                if len > 0
                    && let Some(cached) = cached.as_ref()
                {
                    cached
                        .writeback_dirty_pages_in_range(start..start.saturating_add(len as u64))?;
                }
                let result = src.write_to(&mut axio::write_fn(|buf| {
                    loc.entry()
                        .as_file()?
                        .write_at(buf, offset)
                        .inspect(|written| {
                            offset += *written as u64;
                        })
                }));
                if offset > start {
                    loc.grow_cached_len_if_present(offset);
                    if let Some(cached) = cached {
                        cached.invalidate_clean_pages_in_range(start..offset);
                    }
                }
                let written = result?;
                Ok(written)
            }
        }
    }

    pub fn readahead(&self, offset: u64, count: usize) -> VfsResult<()> {
        match self {
            Self::Cached(cached) => cached.readahead(offset, count),
            Self::Direct(_) => Ok(()),
        }
    }

    /// Returns a stable reference to one aligned cached page. Direct backends
    /// intentionally fall back to copying because they have no cache-owned
    /// frame to pin.
    pub fn page_ref_at(&self, offset: u64) -> VfsResult<Option<FolioPageRef>> {
        if !offset.is_multiple_of(PAGE_SIZE as u64) {
            return Ok(None);
        }
        let Ok(pn) = u32::try_from(offset / PAGE_SIZE as u64) else {
            return Ok(None);
        };
        match self {
            Self::Cached(cached) => {
                if let Some(page) = cached.page_ref_if_cached(pn) {
                    Ok(Some(page))
                } else {
                    cached.fault_page_ref(pn).map(Some)
                }
            }
            Self::Direct(_) => Ok(None),
        }
    }

    /// Returns an already resident aligned cache page without issuing I/O.
    /// This is used by mmap fault-around: the faulting page may perform
    /// readahead, while neighboring PTEs are installed only for pages that are
    /// ready by the time the address-space lock is reacquired.
    pub fn cached_page_ref_at(&self, offset: u64) -> Option<FolioPageRef> {
        if !offset.is_multiple_of(PAGE_SIZE as u64) {
            return None;
        }
        let pn = u32::try_from(offset / PAGE_SIZE as u64).ok()?;
        match self {
            Self::Cached(cached) => cached.page_ref_if_cached(pn),
            Self::Direct(_) => None,
        }
    }

    pub fn with_valid_page_ref<R>(
        &self,
        offset: u64,
        page: &FolioPageRef,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        if !offset.is_multiple_of(PAGE_SIZE as u64) {
            return None;
        }
        let pn = u32::try_from(offset / PAGE_SIZE as u64).ok()?;
        match self {
            Self::Cached(cached) => cached.with_valid_page_ref(pn, page, false, commit),
            Self::Direct(_) => None,
        }
    }

    /// Validates a pinned cached folio once for a short multi-page PTE commit.
    pub fn with_valid_folio_ref<R>(
        &self,
        page: &FolioPageRef,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        match self {
            Self::Cached(cached) => cached.with_valid_folio_ref(page, commit),
            Self::Direct(_) => None,
        }
    }

    pub fn len(&self) -> VfsResult<u64> {
        match self {
            Self::Cached(cached) => cached.len(),
            Self::Direct(loc) => loc.len(),
        }
    }

    pub fn metadata(&self) -> VfsResult<Metadata> {
        match self {
            Self::Cached(cached) => cached.metadata(),
            Self::Direct(loc) => loc.metadata(),
        }
    }

    pub fn record_access(&self, atime: Duration, strict: bool) -> bool {
        match self {
            Self::Cached(cached) => cached.record_access(atime, strict).unwrap_or(false),
            Self::Direct(loc) => loc.record_access(atime, strict).unwrap_or(false),
        }
    }

    pub fn record_modification(&self, mtime: Duration) -> bool {
        self.location().record_modification(mtime).unwrap_or(false)
    }

    pub fn append(&self, mut src: impl Read + IoBuf) -> VfsResult<(usize, u64)> {
        match self {
            Self::Cached(cached) => cached.append(src),
            Self::Direct(loc) => {
                let cached = CachedFile::get_existing(loc);
                if let Some(cached) = cached.as_ref() {
                    cached.sync(true)?;
                }
                let mut start = None;
                let mut end = 0;
                let result = src.write_to(&mut axio::write_fn(|buf| {
                    loc.entry().as_file()?.append(buf).map(|(n, offset)| {
                        if n > 0 && start.is_none() {
                            start = Some(offset.saturating_sub(n as u64));
                        }
                        end = offset;
                        n
                    })
                }));
                if let Some(start) = start {
                    loc.set_cached_len_if_present(end);
                    if let Some(cached) = cached {
                        cached.invalidate_clean_pages_in_range(start..end);
                    }
                }
                let written = result?;
                Ok((written, end))
            }
        }
    }

    pub fn location(&self) -> &Location {
        match self {
            Self::Cached(cached) => cached.location(),
            Self::Direct(loc) => loc,
        }
    }

    pub fn sync(&self, data_only: bool) -> VfsResult<()> {
        match self {
            Self::Cached(cached) => cached.sync(data_only),
            Self::Direct(loc) => loc.sync(data_only),
        }
    }

    pub fn set_len(&self, len: u64) -> VfsResult<()> {
        match self {
            Self::Cached(cached) => cached.set_len(len),
            Self::Direct(loc) => {
                if let Some(cached) = CachedFile::get_existing(loc) {
                    return cached.set_len(len);
                }
                loc.entry().as_file()?.set_len(len)?;
                loc.set_cached_len(len, false)?;
                Ok(())
            }
        }
    }
}

/// Provides `std::fs::File`-like interface.
pub struct File {
    inner: FileBackend,
    flags: FileFlags,
    position: Option<Mutex<u64>>,
    #[cfg(feature = "times")]
    access_flags: AtomicU8,
}

impl File {
    pub fn new(inner: FileBackend, flags: FileFlags) -> Self {
        let position = if inner.location().flags().contains(NodeFlags::STREAM) {
            None
        } else {
            Some(Mutex::new(if flags.contains(FileFlags::APPEND) {
                inner.len().unwrap_or_default()
            } else {
                0
            }))
        };
        Self {
            inner,
            flags,
            position,
            #[cfg(feature = "times")]
            access_flags: AtomicU8::new(0),
        }
    }

    #[cfg(feature = "times")]
    fn mark_access_flag(&self, flag: u8) {
        if self.access_flags.load(Ordering::Relaxed) & flag == 0 {
            self.access_flags.fetch_or(flag, Ordering::Relaxed);
        }
    }

    pub fn open(context: &FsContext, path: impl AsRef<Path>) -> VfsResult<Self> {
        OpenOptions::new()
            .read(true)
            .open(context, path.as_ref())
            .and_then(OpenResult::into_file)
    }

    pub fn create(context: &FsContext, path: impl AsRef<Path>) -> VfsResult<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(context, path.as_ref())
            .and_then(OpenResult::into_file)
    }

    pub fn access(&self, flags: FileFlags) -> VfsResult<&FileBackend> {
        if self.flags.contains(flags) && !self.is_path() {
            Ok(&self.inner)
        } else {
            Err(VfsError::BadFileDescriptor)
        }
    }

    pub fn is_path(&self) -> bool {
        self.flags.contains(FileFlags::PATH)
    }

    pub fn flags(&self) -> FileFlags {
        self.flags
    }

    pub fn backend(&self) -> VfsResult<&FileBackend> {
        self.access(FileFlags::empty())?;
        Ok(&self.inner)
    }

    pub fn location(&self) -> &Location {
        self.inner.location()
    }

    pub fn len(&self) -> VfsResult<u64> {
        self.inner.len()
    }

    pub fn metadata(&self) -> VfsResult<Metadata> {
        self.inner.metadata()
    }

    pub fn record_access(&self, atime: Duration, strict: bool) -> bool {
        self.inner.record_access(atime, strict)
    }

    pub fn record_modification(&self, mtime: Duration) -> bool {
        self.inner.record_modification(mtime)
    }

    /// Reads a number of bytes starting from a given offset.
    pub fn read_at(&self, dst: impl Write + IoBufMut, offset: u64) -> VfsResult<usize> {
        let read = self.access(FileFlags::READ)?.read_at(dst, offset)?;
        #[cfg(feature = "times")]
        if read > 0 {
            self.mark_access_flag(1);
        }
        Ok(read)
    }

    /// Writes a number of bytes starting from a given offset.
    pub fn write_at(&self, src: impl Read + IoBuf, offset: u64) -> VfsResult<usize> {
        let written = self.access(FileFlags::WRITE)?.write_at(src, offset)?;
        #[cfg(feature = "times")]
        if written > 0 {
            self.mark_access_flag(2);
        }
        Ok(written)
    }

    /// Attempts to sync OS-internal file content and metadata to disk.
    ///
    /// If `data_only` is `true`, only the file data is synced, not the
    /// metadata.
    pub fn sync(&self, data_only: bool) -> VfsResult<()> {
        self.access(FileFlags::empty())?;
        self.inner.sync(data_only)
    }

    pub fn read(&self, dst: impl Write + IoBufMut) -> axio::Result<usize> {
        let read = if let Some(pos) = self.position.as_ref() {
            let mut pos = pos.lock();
            self.access(FileFlags::READ)?
                .read_at(dst, *pos)
                .inspect(|n| {
                    *pos += *n as u64;
                })
        } else {
            self.access(FileFlags::READ)?.read_at(dst, 0)
        }?;
        #[cfg(feature = "times")]
        if read > 0 {
            self.mark_access_flag(1);
        }
        Ok(read)
    }

    pub fn write(&self, src: impl Read + IoBuf) -> axio::Result<usize> {
        let written = if let Some(pos) = self.position.as_ref() {
            let mut pos = pos.lock();
            if let Ok(f) = self.access(FileFlags::APPEND) {
                f.append(src).map(|(written, new_size)| {
                    *pos = new_size;
                    written
                })
            } else {
                self.access(FileFlags::WRITE)?
                    .write_at(src, *pos)
                    .inspect(|n| {
                        *pos += *n as u64;
                    })
            }
        } else {
            self.access(FileFlags::WRITE)?.write_at(src, 0)
        }?;
        #[cfg(feature = "times")]
        if written > 0 {
            self.mark_access_flag(2);
        }
        Ok(written)
    }

    pub fn flush(&self) -> axio::Result {
        self.access(FileFlags::empty())?;
        Ok(())
    }
}

impl Read for &File {
    fn read(&mut self, buf: &mut [u8]) -> axio::Result<usize> {
        (*self).read(buf)
    }
}

impl Write for &File {
    fn write(&mut self, buf: &[u8]) -> axio::Result<usize> {
        (*self).write(buf)
    }

    fn flush(&mut self) -> axio::Result {
        (*self).flush()
    }
}

impl Seek for &File {
    fn seek(&mut self, pos: SeekFrom) -> axio::Result<u64> {
        self.access(FileFlags::empty())?;

        if let Some(guard) = self.position.as_ref() {
            let mut guard = guard.lock();
            let new_pos = match pos {
                SeekFrom::Start(pos) => pos,
                SeekFrom::End(off) => {
                    let size = self.access(FileFlags::empty())?.len()?;
                    size.checked_add_signed(off).ok_or(VfsError::InvalidInput)?
                }
                SeekFrom::Current(off) => guard
                    .checked_add_signed(off)
                    .ok_or(VfsError::InvalidInput)?,
            };
            *guard = new_pos;
            Ok(new_pos)
        } else {
            Ok(0)
        }
    }
}

impl Pollable for File {
    fn poll(&self) -> IoEvents {
        self.inner.location().poll()
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        self.inner.location().register(context, events)
    }
}

#[cfg(feature = "times")]
impl Drop for File {
    fn drop(&mut self) {
        let flags = self.access_flags.load(Ordering::Relaxed);
        if flags != 0 {
            let now = axhal::time::realtime_time();
            let mut update = axfs_ng_vfs::MetadataUpdate::default();
            if flags & 1 != 0 && !self.inner.record_access(now, false) {
                update.atime = Some(now);
            }
            if flags & 2 != 0 && !self.inner.record_modification(now) {
                update.mtime = Some(now);
                update.ctime = Some(now);
            }
            if (update.atime.is_some() || update.mtime.is_some())
                && let Err(err) = self.inner.location().update_metadata(update)
            {
                warn!("Failed to update file times on drop: {err:?}");
            }
        }
    }
}
