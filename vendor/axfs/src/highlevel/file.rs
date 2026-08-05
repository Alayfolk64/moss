use alloc::{
    boxed::Box,
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
#[cfg(feature = "times")]
use core::sync::atomic::AtomicU8;
use core::{
    num::NonZeroUsize,
    ops::Range,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
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
use intrusive_collections::{LinkedList, LinkedListAtomicLink, intrusive_adapter};
use lru::LruCache;
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
                cached_metadata(&loc)?.node_type,
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
const DEFAULT_LOCAL_FILE_CACHE_PAGES: usize = 256;
const MAX_RETAINED_FILE_CACHE_RECORDS: usize = 64;
const MAX_READAHEAD_BYTES: usize = DEFAULT_LOCAL_FILE_CACHE_PAGES * PAGE_SIZE;
const DEFERRED_FILE_LEN_THRESHOLD: u64 = 64 * 1024;
const WRITEBACK_BATCH_PAGES: usize = 16;
const WRITEBACK_BATCH_BYTES: usize = WRITEBACK_BATCH_PAGES * PAGE_SIZE;
static ZERO_PAGE: [u8; PAGE_SIZE] = [0; PAGE_SIZE];

#[derive(Debug)]
pub struct PageCache {
    addr: VirtAddr,
    dirty: bool,
}

impl PageCache {
    fn new() -> VfsResult<Self> {
        let addr = global_allocator()
            .alloc_pages(1, PAGE_SIZE, UsageKind::PageCache)
            .inspect_err(|err| {
                warn!("Failed to allocate page cache: {:?}", err);
            })
            .map_err(|_| VfsError::NoMemory)?;
        Ok(Self {
            addr: addr.into(),
            dirty: false,
        })
    }

    pub fn paddr(&self) -> PhysAddr {
        virt_to_phys(self.addr)
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn data(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.addr.as_mut_ptr(), PAGE_SIZE) }
    }
}

impl Drop for PageCache {
    fn drop(&mut self) {
        if self.dirty {
            warn!("dirty page dropped without flushing");
        }
        global_allocator().dealloc_pages(self.addr.as_usize(), 1, UsageKind::PageCache);
    }
}

struct EvictListener {
    listener: Arc<dyn Fn(u32, PhysAddr, bool) -> bool + Send + Sync>,
    owner: Option<usize>,
    link: LinkedListAtomicLink,
}

intrusive_adapter!(EvictListenerAdapter = Box<EvictListener>: EvictListener { link: LinkedListAtomicLink });

struct PendingEviction {
    /// Distinguishes repeated detach/restore lifecycles of the same page and
    /// physical frame.  A delayed completion token may only finish the exact
    /// lifecycle which created it.
    generation: usize,
    page: PageCache,
    /// A normal cache replacement can be cancelled if this page is accessed
    /// again before invalidation finishes.  Truncate/invalidate/discard pages
    /// must not be resurrected.
    reusable: bool,
    /// True until every registered mapping has removed this exact physical
    /// page and completed its remote TLB invalidation.
    needs_invalidation: bool,
    /// Dirty pages detached by sync are put back into the authoritative LRU
    /// after mappings have been revoked and final writeback has completed.
    restore_after_writeback: bool,
}

/// Stable identity for a page detached from the LRU but still pinned by the
/// file cache until all mappings and final writeback have completed.
#[derive(Clone, Copy)]
pub struct PageEviction {
    pn: u32,
    paddr: PhysAddr,
    generation: usize,
}

impl PageEviction {
    pub const fn page_number(self) -> u32 {
        self.pn
    }

    pub const fn paddr(self) -> PhysAddr {
        self.paddr
    }
}

/// Ordered phases of a private-mapping EOF-shrink callback.  Every observer
/// completes `Protect` before any observer receives `Apply`, establishing a
/// cross-address-space write/TLB barrier for fork-shared anonymous frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileLengthChangePhase {
    Protect,
    Apply,
}

struct CachedFileShared {
    page_cache: Mutex<LruCache<u32, PageCache>>,
    evict_listeners: Mutex<LinkedList<EvictListenerAdapter>>,
    /// Pages whose mappings could not all be invalidated immediately.  Keeping
    /// the frames here prevents stale PTEs from becoming use-after-free aliases.
    pending_evictions: Mutex<BTreeMap<u32, PendingEviction>>,
    next_eviction_generation: AtomicUsize,
    /// Keeps detached pages from being reloaded by the same inode operation.
    cache_op: Mutex<()>,
    /// Serializes complete dirty-page synchronization lifecycles.  In
    /// particular, a second fsync must not observe an empty LRU while the first
    /// fsync still owns dirty pages in `pending_evictions`.
    writeback_op: Mutex<()>,
    /// Only one thread can append to the file at a time, while multiple writers
    /// are permitted.
    append_lock: RwLock<()>,
    /// Range whose cached pages are being revoked for an EOF change.  It is
    /// published before the first writeback and remains visible until the
    /// backing EOF and cache state have both committed (or rolled back).
    length_change: Mutex<Option<Range<u64>>>,
    /// Monotonic identity of the file-length state.  Unlike the transient
    /// marker above, this also detects a complete shrink-then-regrow cycle
    /// between a private fault's file read and PTE commit.
    length_generation: AtomicUsize,
    /// MAP_PRIVATE mappings keep anonymous frames which are no longer tied to
    /// page-cache eviction.  A successful shrink must still revoke their PTEs
    /// beyond the new EOF before the length-change transaction becomes
    /// invisible to faults.
    length_change_listeners:
        Mutex<BTreeMap<usize, Arc<dyn Fn(Range<u64>, FileLengthChangePhase) + Send + Sync>>>,
    next_length_change_listener: AtomicUsize,
}

impl CachedFileShared {
    pub fn new() -> Self {
        Self {
            page_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(DEFAULT_LOCAL_FILE_CACHE_PAGES).unwrap(),
            )),
            evict_listeners: Mutex::new(LinkedList::default()),
            pending_evictions: Mutex::new(BTreeMap::new()),
            next_eviction_generation: AtomicUsize::new(1),
            cache_op: Mutex::new(()),
            writeback_op: Mutex::new(()),
            append_lock: RwLock::new(()),
            length_change: Mutex::new(None),
            length_generation: AtomicUsize::new(0),
            length_change_listeners: Mutex::new(BTreeMap::new()),
            next_length_change_listener: AtomicUsize::new(1),
        }
    }

    pub fn new_unbounded() -> Self {
        Self {
            page_cache: Mutex::new(LruCache::unbounded()),
            evict_listeners: Mutex::new(LinkedList::default()),
            pending_evictions: Mutex::new(BTreeMap::new()),
            next_eviction_generation: AtomicUsize::new(1),
            cache_op: Mutex::new(()),
            writeback_op: Mutex::new(()),
            append_lock: RwLock::new(()),
            length_change: Mutex::new(None),
            length_generation: AtomicUsize::new(0),
            length_change_listeners: Mutex::new(BTreeMap::new()),
            next_length_change_listener: AtomicUsize::new(1),
        }
    }
}

struct LengthChangeGuard<'a> {
    shared: &'a CachedFileShared,
}

impl Drop for LengthChangeGuard<'_> {
    fn drop(&mut self) {
        let old = self.shared.length_change.lock().take();
        debug_assert!(old.is_some(), "length-change guard lost its range");
    }
}

type FileCacheKey = (usize, u64);

struct CachedFileRecord {
    key: FileCacheKey,
    location: Location,
    shared: Arc<CachedFileShared>,
    in_memory: bool,
    removed: AtomicBool,
    active_handles: AtomicUsize,
}

static FILE_CACHES: spin::Lazy<RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>>> =
    spin::Lazy::new(|| RwLock::new(BTreeMap::new()));

pub struct CachedFile {
    inner: Location,
    record: Arc<CachedFileRecord>,
    /// Whether this counted reference belongs to an open file/mmap user.
    /// Internal registry snapshots are counted too, but only run last-reference
    /// cleanup for records which have already been removed from the namespace.
    tracked: bool,
}

impl Clone for CachedFile {
    fn clone(&self) -> Self {
        self.record.active_handles.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: self.inner.clone(),
            record: self.record.clone(),
            tracked: self.tracked,
        }
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

    pub fn get_or_create(location: Location) -> Self {
        let in_memory = location.filesystem().name() == "tmpfs";
        let key = Self::file_cache_key(&location);

        // `user_data -> FILE_CACHES` is the registry lock order used by both
        // lookup and removal.  A weak pointer is usable only while the same
        // record is still the current global entry; otherwise a removal racing
        // this lookup could split one inode into two cache/lock domains.
        let mut user_data = location.user_data();
        let mut caches = FILE_CACHES.write();
        let weak_record = user_data.get::<FileUserData>().and_then(|it| it.get());
        let record = weak_record
            .filter(|record| {
                caches
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(current, record))
            })
            .unwrap_or_else(|| {
                caches
                    .entry(key)
                    .or_insert_with(|| {
                        let shared = if in_memory {
                            Arc::new(CachedFileShared::new_unbounded())
                        } else {
                            Arc::new(CachedFileShared::new())
                        };
                        Arc::new(CachedFileRecord {
                            key,
                            location: location.clone(),
                            shared,
                            in_memory,
                            removed: AtomicBool::new(false),
                            active_handles: AtomicUsize::new(0),
                        })
                    })
                    .clone()
            });
        user_data.insert(FileUserData::Weak(Arc::downgrade(&record)));
        // Publish the user before releasing the registry locks. A concurrent
        // finalizer therefore cannot retire the record between lookup and the
        // handle becoming visible.
        record.active_handles.fetch_add(1, Ordering::Relaxed);
        drop(caches);
        drop(user_data);

        Self {
            inner: location,
            record,
            tracked: true,
        }
    }

    fn remove_global_record_if_unused(&self) -> bool {
        self.remove_global_record_with_active_count(0)
    }

    fn remove_global_record_if_only_snapshot(&self) -> bool {
        debug_assert!(!self.tracked);
        self.remove_global_record_with_active_count(1)
    }

    fn remove_global_record_with_active_count(&self, expected_active: usize) -> bool {
        // Match `get_or_create`'s registry order. Holding `user_data` prevents
        // a new tracked handle from being published between the active-user
        // check and global removal. Registry snapshots increment the same
        // counter before releasing FILE_CACHES' read side.
        let mut user_data = self.inner.user_data();
        let mut caches = FILE_CACHES.write();
        let is_current = caches
            .get(&self.record.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.record));
        if !is_current
            || self.record.active_handles.load(Ordering::Acquire) != expected_active
        {
            return false;
        }

        caches.remove(&self.record.key);
        if user_data
            .get::<FileUserData>()
            .and_then(|it| it.get())
            .is_some_and(|record| Arc::ptr_eq(&record, &self.record))
        {
            user_data.remove::<FileUserData>();
        }
        true
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

    fn shared(&self) -> &CachedFileShared {
        self.record.shared.as_ref()
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

    fn from_record(record: &Arc<CachedFileRecord>) -> Self {
        // Callers must invoke this while holding FILE_CACHES for reading.  The
        // increment is then published before a finalizer can acquire the write
        // side and retire the registry entry.
        record.active_handles.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: record.location.clone(),
            record: record.clone(),
            tracked: false,
        }
    }

    fn get_existing(key: FileCacheKey) -> Option<Self> {
        let caches = FILE_CACHES.read();
        caches.get(&key).map(Self::from_record)
    }

    fn begin_length_change(&self, range: Range<u64>) -> LengthChangeGuard<'_> {
        let _cache_guard = self.shared().cache_op.lock();
        let mut active = self.shared().length_change.lock();
        assert!(active.is_none(), "overlapping file length changes");
        self.shared()
            .length_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .expect("file length generation exhausted");
        *active = Some(range);
        LengthChangeGuard {
            shared: self.shared(),
        }
    }

    fn length_change_blocks_page(&self, pn: u32) -> bool {
        self.shared()
            .length_change
            .lock()
            .as_ref()
            .is_some_and(|range| Self::page_overlaps_range(pn, range))
    }

    /// Takes one coherent snapshot of the transient marker and persistent
    /// generation.  `None` means the requested page is currently covered by a
    /// length-change transaction and must not be read or committed.
    fn length_generation_for_page(&self, pn: u32) -> Option<usize> {
        let active = self.shared().length_change.lock();
        if active
            .as_ref()
            .is_some_and(|range| Self::page_overlaps_range(pn, range))
        {
            None
        } else {
            Some(self.shared().length_generation.load(Ordering::Acquire))
        }
    }

    fn add_length_change_listener<F>(&self, listener: F) -> usize
    where
        F: Fn(Range<u64>, FileLengthChangePhase) + Send + Sync + 'static,
    {
        let handle = self
            .shared()
            .next_length_change_listener
            .fetch_add(1, Ordering::Relaxed);
        assert_ne!(handle, 0, "file length-change listener handles exhausted");
        let previous = self
            .shared()
            .length_change_listeners
            .lock()
            .insert(handle, Arc::new(listener));
        assert!(previous.is_none(), "duplicate file length-change listener");
        handle
    }

    fn remove_length_change_listener(&self, handle: usize) {
        let removed = self
            .shared()
            .length_change_listeners
            .lock()
            .remove(&handle);
        debug_assert!(removed.is_some(), "unknown file length-change listener");
    }

    fn notify_length_change_listeners(&self, range: Range<u64>) {
        // Listener destruction removes itself from this table.  Snapshot the
        // callbacks so an address-space teardown cannot deadlock on the list.
        let listeners = self
            .shared()
            .length_change_listeners
            .lock()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in &listeners {
            listener(range.clone(), FileLengthChangePhase::Protect);
        }
        for listener in listeners {
            listener(range.clone(), FileLengthChangePhase::Apply);
        }
    }

    pub fn add_evict_listener<F>(&self, listener: F) -> usize
    where
        F: Fn(u32) + Send + Sync + 'static,
    {
        self.add_evict_listener_checked(usize::MAX, move |pn, _, _| {
            listener(pn);
            true
        })
    }

    /// Registers an eviction listener that confirms page-table removal and
    /// synchronous remote TLB invalidation for the exact physical page.
    pub fn add_evict_listener_checked<F>(&self, owner: usize, listener: F) -> usize
    where
        F: Fn(u32, PhysAddr, bool) -> bool + Send + Sync + 'static,
    {
        let pointer = Box::new(EvictListener {
            listener: Arc::new(listener),
            owner: (owner != usize::MAX).then_some(owner),
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

    fn notify_evict_listeners_except(
        &self,
        pn: u32,
        paddr: PhysAddr,
        skip_owner: Option<usize>,
        wait: bool,
    ) -> bool {
        // Never invoke an address-space callback while holding the listener
        // list lock: dropping an mmap backend removes its listener.
        let listeners = self
            .shared()
            .evict_listeners
            .lock()
            .iter()
            .filter_map(|listener| {
                (listener.owner != skip_owner).then(|| listener.listener.clone())
            })
            .collect::<Vec<_>>();
        // Do not short-circuit: even if one mapping reports a paging error,
        // every other mapping should still make forward progress.
        let mut complete = true;
        for listener in listeners {
            complete = listener(pn, paddr, wait) && complete;
        }
        complete
    }

    /// Notifies every mapping outside `skip_owner` about a detached page.
    ///
    /// The cache keeps the detached page alive in its pending table.  The
    /// caller must complete its skipped owner before confirming the token.
    pub fn notify_evicted_page_except(
        &self,
        eviction: PageEviction,
        skip_owner: usize,
    ) -> bool {
        self.notify_evict_listeners_except(eviction.pn, eviction.paddr, Some(skip_owner), false)
    }

    /// Notifies every mapping about a detached page.
    ///
    /// This is used by error cleanup which only has a page-table handle for
    /// one mapping, not the full current address space.  If that address space
    /// is still locked, its checked listeners return `false` and keep the page
    /// pinned until a later retry can remove every sibling mapping.
    pub fn notify_evicted_page(&self, eviction: PageEviction) -> bool {
        self.notify_evict_listeners_except(eviction.pn, eviction.paddr, None, false)
    }

    fn begin_evicted_page(
        &self,
        pn: u32,
        page: PageCache,
        reusable: bool,
        restore_after_writeback: bool,
    ) -> PageEviction {
        let paddr = page.paddr();
        let generation = self
            .shared()
            .next_eviction_generation
            .fetch_add(1, Ordering::Relaxed);
        assert_ne!(generation, 0, "page eviction generation exhausted");
        let mut pending = self.shared().pending_evictions.lock();
        assert!(
            !pending.contains_key(&pn),
            "duplicate pending eviction for page {pn}"
        );
        pending.insert(
            pn,
            PendingEviction {
                generation,
                page,
                reusable,
                needs_invalidation: true,
                restore_after_writeback,
            },
        );
        PageEviction {
            pn,
            paddr,
            generation,
        }
    }

    fn eviction_matches(entry: &PendingEviction, eviction: PageEviction) -> bool {
        entry.generation == eviction.generation && entry.page.paddr() == eviction.paddr
    }

    /// Finalizes exact detached-page lifecycles after their PTE/TLB barrier.
    ///
    /// `cache_op` keeps the pages authoritative while they are temporarily
    /// moved out of `pending_evictions` for batched I/O.  On writeback failure
    /// every page is reinserted with `needs_invalidation = false`, so a retry
    /// cannot free or duplicate the frame.  Sync-detached pages are restored
    /// to the LRU only after their dirty bit has been cleared by successful
    /// final writeback.
    fn finish_evicted_writeback_batch(
        &self,
        evictions: &[PageEviction],
    ) -> VfsResult<Vec<PageEviction>> {
        if evictions.is_empty() {
            return Ok(Vec::new());
        }

        let _cache_guard = self.shared().cache_op.lock();
        let mut detached = {
            let mut pending = self.shared().pending_evictions.lock();
            let mut detached = Vec::new();
            for eviction in evictions {
                let Some(entry) = pending.get(&eviction.pn) else {
                    // A normal reusable eviction may have been cancelled and
                    // restored by a concurrent access.
                    continue;
                };
                if !Self::eviction_matches(entry, *eviction) {
                    // This is a stale token from an older lifecycle of the
                    // same page/frame.  It must not affect the current entry.
                    continue;
                }
                let mut entry = pending
                    .remove(&eviction.pn)
                    .expect("matching pending eviction disappeared");
                entry.needs_invalidation = false;
                detached.push((*eviction, entry));
            }
            detached
        };

        let writeback_result = (|| {
            if !self.in_memory() && detached.iter().any(|(_, entry)| entry.page.dirty) {
                let file = self.inner().entry().as_file()?;
                let file_len = self.commit_len()?;
                Self::writeback_dirty_pages(file, file_len, &mut detached)?;
            }
            Ok(())
        })();

        if let Err(err) = writeback_result {
            let mut pending = self.shared().pending_evictions.lock();
            for (eviction, mut entry) in detached {
                // The PTE/TLB barrier already completed.  Let a fault restore
                // the still-dirty page instead of blocking forever if storage
                // keeps returning an error.
                entry.reusable = true;
                let previous = pending.insert(eviction.pn, entry);
                assert!(previous.is_none(), "duplicate pending eviction after writeback failure");
            }
            return Err(err);
        }

        let mut displaced = Vec::new();
        let mut cache = self.shared().page_cache.lock();
        for (eviction, entry) in detached {
            if !entry.restore_after_writeback {
                continue;
            }
            if cache.len() == cache.cap().get()
                && let Some((pn, page)) = cache.pop_lru()
            {
                displaced.push((pn, page));
            }
            cache.put(eviction.pn, entry.page);
        }
        drop(cache);

        Ok(displaced
            .into_iter()
            .map(|(pn, page)| self.begin_evicted_page(pn, page, true, false))
            .collect())
    }

    fn finish_evicted_writeback_with_wait(
        &self,
        eviction: PageEviction,
        wait: bool,
    ) -> VfsResult<()> {
        let mut work = self.finish_evicted_writeback_batch(&[eviction])?;
        while let Some(displaced) = work.pop() {
            let mappings_invalidated = self.notify_evict_listeners_except(
                displaced.pn,
                displaced.paddr,
                None,
                wait,
            );
            if !mappings_invalidated {
                return Err(VfsError::WouldBlock);
            }
            work.extend(self.finish_evicted_writeback_batch(&[displaced])?);
        }
        Ok(())
    }

    /// Completes a caller-owned eviction after the caller has dealt with its
    /// own locked address space.  The PageCache remains centrally pinned, so
    /// failure cannot free a frame referenced by a PTE or remote TLB.
    pub fn complete_evicted_page(
        &self,
        eviction: PageEviction,
        mappings_invalidated: bool,
    ) -> VfsResult<()> {
        if !mappings_invalidated {
            return Err(VfsError::WouldBlock);
        }
        // This entry point can be called while an mmap population owns an
        // address-space lock, so waiting behind fsync would create an ABBA
        // cycle.  Opportunistic completion is still serialized when the lock
        // is immediately available; otherwise the pinned token is retried by
        // the next non-address-space owner.
        let Some(_writeback_guard) = self.shared().writeback_op.try_lock() else {
            return Err(VfsError::WouldBlock);
        };
        // A caller which owns one address-space lock must not block while a
        // restored page displaces another cache entry: another listener could
        // own its address space in the opposite order.
        self.finish_evicted_writeback_with_wait(eviction, false)
    }

    fn finish_evicted_page_with_wait(
        &self,
        eviction: PageEviction,
        wait: bool,
    ) -> VfsResult<()> {
        let mappings_invalidated =
            self.notify_evict_listeners_except(eviction.pn, eviction.paddr, None, wait);
        if !mappings_invalidated {
            return Err(VfsError::WouldBlock);
        }
        self.finish_evicted_writeback_with_wait(eviction, wait)
    }

    fn retry_pending_evictions_matching(&self, include_sync: bool) -> VfsResult<()> {
        let candidates = self
            .shared()
            .pending_evictions
            .lock()
            .iter()
            .filter_map(|(pn, entry)| {
                (include_sync || !entry.restore_after_writeback).then_some((
                    PageEviction {
                        pn: *pn,
                        paddr: entry.page.paddr(),
                        generation: entry.generation,
                    },
                    entry.needs_invalidation,
                ))
            })
            .collect::<Vec<_>>();
        let mut first_error = None;
        for (eviction, needs_invalidation) in candidates {
            if needs_invalidation
                && !self.notify_evict_listeners_except(
                    eviction.pn,
                    eviction.paddr,
                    None,
                    include_sync,
                )
            {
                first_error.get_or_insert(VfsError::WouldBlock);
                continue;
            }
            if let Err(err) = self.finish_evicted_writeback_with_wait(eviction, include_sync) {
                warn!(
                    "Failed to finish pending page eviction {}: {err:?}",
                    eviction.pn
                );
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn retry_pending_evictions(&self) -> VfsResult<()> {
        self.retry_pending_evictions_matching(true)
    }

    fn retry_pending_normal_evictions(&self) -> VfsResult<()> {
        // This opportunistic path may be entered while mmap population owns an
        // address-space lock, so it must never wait behind fsync (which can be
        // waiting for that same address space).  If the serialization lock is
        // immediately available, owning it closes the dirty-token visibility
        // gap with fsync/truncate while the token is detached for writeback.
        let Some(_writeback_guard) = self.shared().writeback_op.try_lock() else {
            return Ok(());
        };
        self.retry_pending_evictions_matching(false)
    }

    /// Returns with `cache_op` held only after every pending token visible at
    /// the same serialization point has completed.  The caller must already
    /// own `writeback_op`; this closes the scan-then-lock gap where ordinary
    /// cache insertion could otherwise create a dirty pending page between an
    /// fsync/truncate pending scan and its LRU scan.
    fn lock_cache_after_draining_pending(&self) -> VfsResult<MutexGuard<'_, ()>> {
        loop {
            self.retry_pending_evictions()?;
            let cache_guard = self.shared().cache_op.lock();
            if self.shared().pending_evictions.lock().is_empty() {
                return Ok(cache_guard);
            }
            drop(cache_guard);
        }
    }

    fn page_or_insert<'a>(
        &self,
        file: &FileNode,
        cache: &'a mut LruCache<u32, PageCache>,
        pn: u32,
    ) -> VfsResult<(&'a mut PageCache, Option<(u32, PageCache)>)> {
        // TODO: Matching the result of `get_mut` confuses compiler. See
        // https://users.rust-lang.org/t/return-do-not-release-mutable-borrow/55757.
        if cache.contains(&pn) {
            return Ok((cache.get_mut(&pn).unwrap(), None));
        }
        let restored = {
            let mut pending = self.shared().pending_evictions.lock();
            match pending.get(&pn) {
                Some(entry) if !entry.reusable => return Err(VfsError::WouldBlock),
                Some(_) => pending.remove(&pn).map(|entry| entry.page),
                None => None,
            }
        };
        // Prepare the replacement before detaching the old page.  Allocation or
        // I/O failure therefore cannot silently free a still-mapped old frame.
        let page = if let Some(page) = restored {
            page
        } else {
            let mut page = PageCache::new()?;
            page.data().fill(0);
            if !self.in_memory() {
                file.read_at(page.data(), pn as u64 * PAGE_SIZE as u64)?;
            }
            page
        };
        let mut evicted = None;
        if cache.len() == cache.cap().get() {
            // Cache is full, remove the least recently used page.
            if let Some((old_pn, old_page)) = cache.pop_lru() {
                evicted = Some((old_pn, old_page));
            }
        }
        cache.put(pn, page);
        Ok((cache.get_mut(&pn).unwrap(), evicted))
    }

    fn page_or_insert_zeroed<'a>(
        &self,
        file: &FileNode,
        cache: &'a mut LruCache<u32, PageCache>,
        pn: u32,
    ) -> VfsResult<(&'a mut PageCache, Option<(u32, PageCache)>)> {
        if cache.contains(&pn) {
            return Ok((cache.get_mut(&pn).unwrap(), None));
        }
        let restored = {
            let mut pending = self.shared().pending_evictions.lock();
            match pending.get(&pn) {
                Some(entry) if !entry.reusable => return Err(VfsError::WouldBlock),
                Some(_) => pending.remove(&pn).map(|entry| entry.page),
                None => None,
            }
        };
        let page = if let Some(page) = restored {
            page
        } else {
            let mut page = PageCache::new()?;
            page.data().fill(0);
            page
        };
        let mut evicted = None;
        if cache.len() == cache.cap().get() {
            if let Some((old_pn, old_page)) = cache.pop_lru() {
                evicted = Some((old_pn, old_page));
            }
        }
        cache.put(pn, page);
        Ok((cache.get_mut(&pn).unwrap(), evicted))
    }

    fn can_zero_fill_write_page(old_len: u64, page_start: u64, range: &Range<usize>) -> bool {
        if page_start >= old_len {
            return true;
        }
        let old_valid_end = (old_len - page_start).min(PAGE_SIZE as u64) as usize;
        range.start == 0 && range.end >= old_valid_end
    }

    pub fn with_page<R>(&self, pn: u32, f: impl FnOnce(Option<&mut PageCache>) -> R) -> R {
        let _cache_guard = self.shared().cache_op.lock();
        if self.length_change_blocks_page(pn) {
            return f(None);
        }
        let mut pending = self.shared().pending_evictions.lock();
        if pending.get(&pn).is_some_and(|entry| !entry.reusable) {
            return f(None);
        }
        let restored = pending.remove(&pn).map(|entry| entry.page);
        drop(pending);

        let mut cache = self.shared().page_cache.lock();
        let displaced = if let Some(page) = restored {
            let displaced = if cache.len() == cache.cap().get() {
                cache.pop_lru()
            } else {
                None
            };
            cache.put(pn, page);
            displaced
        } else {
            None
        };
        let result = f(cache.get_mut(&pn));
        drop(cache);

        if let Some((old_pn, old_page)) = displaced {
            // The caller may hold an address-space lock, so callbacks cannot
            // run here.  Keep the replacement pinned; the normal retry path
            // will invalidate its mappings after that lock has been dropped.
            let _ = self.begin_evicted_page(old_pn, old_page, true, false);
        }
        result
    }

    /// Returns true while a non-reusable detached page is crossing its
    /// PTE/TLB/writeback barrier.  Page-fault code must drop the address-space
    /// lock and retry instead of remapping that frame during this interval.
    pub fn page_eviction_in_progress(&self, pn: u32) -> bool {
        let _cache_guard = self.shared().cache_op.lock();
        self.length_change_blocks_page(pn)
            || self
                .shared()
                .pending_evictions
                .lock()
                .get(&pn)
                .is_some_and(|entry| !entry.reusable)
    }

    fn with_page_or_insert_impl<R>(
        &self,
        pn: u32,
        zeroed: bool,
        wait_for_pending: bool,
        f: impl FnOnce(&mut PageCache, Option<(u32, &PageCache)>) -> VfsResult<R>,
    ) -> VfsResult<R> {
        let mut f = Some(f);
        loop {
            // Pending normal replacements remain centrally pinned if an address-
            // space lock is busy.  Retry only those tokens here: an active sync
            // still owns `writeback_op` and must finish before ordinary I/O may
            // reuse its detached page.
            let _ = self.retry_pending_normal_evictions();
            let (result, eviction) = {
                let cache_guard = self.shared().cache_op.lock();
                let transaction_blocks_access = self.length_change_blocks_page(pn);
                let pending_blocks_access = self
                    .shared()
                    .pending_evictions
                    .lock()
                    .get(&pn)
                    .is_some_and(|entry| !entry.reusable);
                if wait_for_pending && (transaction_blocks_access || pending_blocks_access) {
                    // Do not wait while holding `cache_op`: the writeback owner
                    // needs it to restore or retire this exact token.  Once the
                    // current owner releases `writeback_op`, retry any lifecycle
                    // which ended with a recoverable listener failure.
                    drop(cache_guard);
                    let _writeback_guard = self.shared().writeback_op.lock();
                    self.retry_pending_evictions()?;
                    continue;
                }
                if transaction_blocks_access || pending_blocks_access {
                    return Err(VfsError::WouldBlock);
                }

                let mut guard = self.shared().page_cache.lock();
                let file = self.inner().entry().as_file()?;
                let (page, evicted) = if zeroed {
                    self.page_or_insert_zeroed(file, &mut guard, pn)?
                } else {
                    self.page_or_insert(file, &mut guard, pn)?
                };
                let evicted_ref = evicted.as_ref().map(|(pn, page)| (*pn, page));
                let result = f
                    .take()
                    .expect("page-cache callback executed more than once")(
                    page,
                    evicted_ref,
                );
                let eviction =
                    evicted.map(|(pn, page)| self.begin_evicted_page(pn, page, true, false));
                (result, eviction)
            };
            let finish_result = if let Some(eviction) = eviction {
                let mappings_invalidated = self.notify_evict_listeners_except(
                    eviction.pn,
                    eviction.paddr,
                    None,
                    false,
                );
                self.complete_evicted_page(eviction, mappings_invalidated)
            } else {
                Ok(())
            };
            return match result {
                Ok(value) => match finish_result {
                    Ok(()) => Ok(value),
                    Err(err) => {
                        // The callback has already consumed input or copied data
                        // into the target page.  A failure while retiring an
                        // unrelated displaced page must not turn that completed
                        // operation into an error which the caller may retry.
                        // The failed page remains pinned and dirty in
                        // `pending_evictions`, so a later operation can retry the
                        // writeback without losing data.
                        warn!("Deferred displaced-page writeback failed: {err:?}");
                        Ok(value)
                    }
                },
                Err(err) => Err(err),
            };
        }
    }

    pub fn with_page_or_insert<R>(
        &self,
        pn: u32,
        f: impl FnOnce(&mut PageCache, Option<(u32, &PageCache)>) -> VfsResult<R>,
    ) -> VfsResult<R> {
        // Address-space population can hold a mapping lock here, so this public
        // form must remain nonblocking to avoid an fsync -> aspace ABBA cycle.
        self.with_page_or_insert_impl(pn, false, false, f)
    }

    /// Inserts a page and returns a stable token for any detached cache page.
    /// The cache retains ownership; mmap uses the token to keep that frame
    /// pinned until PTE removal and remote TLB acknowledgement are complete.
    pub fn with_page_or_insert_deferred<R>(
        &self,
        pn: u32,
        f: impl FnOnce(&mut PageCache) -> R,
    ) -> VfsResult<(R, Option<PageEviction>)> {
        let _cache_guard = self.shared().cache_op.lock();
        if self.length_change_blocks_page(pn) {
            return Err(VfsError::WouldBlock);
        }
        // Recheck EOF at the same serialization point as insertion. A fault
        // may have sampled the old length before waiting behind truncate;
        // without this check it could map a fully out-of-range page after the
        // shorter EOF was already committed.
        if u64::from(pn) * PAGE_SIZE as u64 >= self.len()? {
            return Err(VfsError::Io);
        }
        let mut guard = self.shared().page_cache.lock();
        let file = self.inner().entry().as_file()?;
        let (page, evicted) = self.page_or_insert(file, &mut guard, pn)?;
        let result = f(page);
        let eviction = evicted.map(|(pn, page)| self.begin_evicted_page(pn, page, true, false));
        Ok((result, eviction))
    }

    fn with_pages<T>(
        &self,
        range: Range<u64>,
        page_initial: impl FnOnce(&FileNode) -> VfsResult<T>,
        mut page_each: impl FnMut(T, &mut PageCache, Range<usize>) -> VfsResult<T>,
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
            initial =
                Some(self.with_page_or_insert_impl(pn, false, true, |page, _| {
                    page_each(current, page, page_range)
                })?);
            page_offset = 0;
        }

        Ok(initial.unwrap())
    }

    pub fn read_at(&self, mut dst: impl Write + IoBufMut, offset: u64) -> VfsResult<usize> {
        let _append_guard = self.shared().append_lock.read();
        let len = self.len()?;
        let end = (offset + dst.remaining_mut() as u64).min(len);
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
                dst.write(&page.data()[range.start..range.end])?;
                Ok(read + len)
            },
        )
    }

    /// Reads coherent cached contents without inserting or replacing an LRU
    /// page.  Address-space population uses this fallback while it holds the
    /// structural lock: triggering an eviction there would have to call back
    /// into address spaces and could otherwise create a self/ABBA lock cycle.
    pub fn read_at_no_evict(
        &self,
        mut dst: impl Write + IoBufMut,
        offset: u64,
    ) -> VfsResult<usize> {
        let len = self.len()?;
        let end = (offset + dst.remaining_mut() as u64).min(len);
        if end <= offset {
            return Ok(0);
        }

        let file = self.inner().entry().as_file()?;
        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (offset % PAGE_SIZE as u64) as usize;
        let mut read = 0;

        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let range_end = (end - page_start).min(PAGE_SIZE as u64) as usize;
            let range = page_offset..range_end;

            // Keep the cache lookup and backing-file fallback in one serialized
            // operation.  Otherwise a cached writer could create a newer dirty
            // page after our miss but before the disk read.
            let _cache_guard = self.shared().cache_op.lock();
            if self.length_change_blocks_page(pn) {
                return Err(VfsError::WouldBlock);
            }
            let mut cache = self.shared().page_cache.lock();
            if let Some(page) = cache.get_mut(&pn) {
                dst.write(&page.data()[range.clone()])?;
            } else {
                drop(cache);
                let mut pending = self.shared().pending_evictions.lock();
                if let Some(entry) = pending.get_mut(&pn) {
                    // Both reusable and non-reusable pending pages remain
                    // pinned and contain the newest dirty data.
                    dst.write(&entry.page.data()[range.clone()])?;
                } else {
                    drop(pending);
                    if self.in_memory() {
                        dst.write(&ZERO_PAGE[range.clone()])?;
                    } else {
                        let mut page = Vec::new();
                        page.resize(PAGE_SIZE, 0);
                        file.read_at(&mut page[..], page_start)?;
                        dst.write(&page[range.clone()])?;
                    }
                }
            }

            read += range.end - range.start;
            page_offset = 0;
        }
        Ok(read)
    }

    pub fn readahead(&self, offset: u64, count: usize) -> VfsResult<()> {
        let _append_guard = self.shared().append_lock.read();
        if count == 0 || self.in_memory() {
            return Ok(());
        }

        let len = self.len()?;
        if offset >= len {
            return Ok(());
        }
        let count = count.min(MAX_READAHEAD_BYTES);
        let end = offset.saturating_add(count as u64).min(len);
        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;

        for pn in start_page..end_page {
            self.with_page_or_insert_impl(pn, false, true, |_, _| Ok(()))?;
        }

        Ok(())
    }

    fn read_at_in_memory(
        &self,
        mut dst: impl Write + IoBufMut,
        offset: u64,
        end: u64,
    ) -> VfsResult<usize> {
        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (offset % PAGE_SIZE as u64) as usize;
        let mut read = 0;

        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let range_start = page_offset;
            let range_end = (end - page_start).min(PAGE_SIZE as u64) as usize;
            let len = range_end - range_start;

            loop {
                let cache_guard = self.shared().cache_op.lock();
                if self.length_change_blocks_page(pn) {
                    drop(cache_guard);
                    let _writeback_guard = self.shared().writeback_op.lock();
                    continue;
                }
                let mut guard = self.shared().page_cache.lock();
                if let Some(page) = guard.get_mut(&pn) {
                    dst.write(&page.data()[range_start..range_end])?;
                } else {
                    dst.write(&ZERO_PAGE[range_start..range_end])?;
                }
                break;
            }
            read += len;
            page_offset = 0;
        }

        Ok(read)
    }

    fn write_at_locked(&self, mut buf: impl Read + IoBuf, offset: u64) -> VfsResult<usize> {
        let end = offset + buf.remaining() as u64;
        if end <= offset {
            return Ok(0);
        }
        if self.in_memory() {
            return self.write_at_in_memory(buf, offset, end);
        }

        let old_len = self.len()?;
        if end > old_len {
            if end <= DEFERRED_FILE_LEN_THRESHOLD {
                let file = self.inner().entry().as_file()?;
                file.set_len(end)?;
                // Backing EOF has committed.  Update an existing cache
                // infallibly; if another thread invalidated it, the next
                // lookup reads the new backing metadata instead of recreating
                // an old cached EOF through a fallible second operation.
                self.inner().set_cached_len_if_present(end);
            } else {
                self.grow_cached_len(end)?;
            }
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
            self.with_page_or_insert_impl(pn, zeroed, true, |page, _| {
                buf.read(&mut page.data()[range.start..range.end])?;
                page.dirty = true;
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
            self.inner().set_cached_len_if_present(end);
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

            let mut guard = self.shared().page_cache.lock();
            if let Some(page) = guard.get_mut(&pn) {
                buf.read(&mut page.data()[range_start..range_end])?;
            } else {
                let mut data = Vec::new();
                data.resize(len, 0);
                buf.read(&mut data)?;
                if data.iter().any(|byte| *byte != 0) {
                    let mut page = PageCache::new()?;
                    page.data().fill(0);
                    page.data()[range_start..range_end].copy_from_slice(&data);
                    guard.put(pn, page);
                }
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

    /// Restores transactionally detached pages while `cache_op` is held.
    /// Concurrent activity may have filled the freed LRU slots while PTE
    /// callbacks ran, so any displaced page starts its own pinned lifecycle.
    fn restore_detached_pages_locked(
        &self,
        pages: Vec<(u32, PageCache)>,
    ) -> Vec<PageEviction> {
        let mut cache = self.shared().page_cache.lock();
        let mut displaced = Vec::new();
        for (pn, page) in pages {
            debug_assert!(!cache.contains(&pn));
            if cache.len() == cache.cap().get()
                && let Some(entry) = cache.pop_lru()
            {
                displaced.push(entry);
            }
            cache.put(pn, page);
        }
        drop(cache);
        displaced
            .into_iter()
            .map(|(pn, page)| self.begin_evicted_page(pn, page, true, false))
            .collect()
    }

    /// Detaches every cached page overlapping `range`, revokes all exact PTEs
    /// and returns the still-owned frames with writeback/cache serialization
    /// held. No backing-file mutation has happened yet. If a listener fails,
    /// the original frames are restored before the error is returned.
    fn lock_pages_for_len_change(
        &self,
        range: Range<u64>,
    ) -> VfsResult<(
        MutexGuard<'_, ()>,
        MutexGuard<'_, ()>,
        Vec<(u32, PageCache)>,
    )> {
        let writeback_guard = self.shared().writeback_op.lock();
        self.writeback_dirty_pages_in_range_locked(&range)?;

        let cache_guard = self.lock_cache_after_draining_pending()?;
        let evictions = {
            let mut cache = self.shared().page_cache.lock();
            let keys = cache
                .iter()
                .map(|(pn, _)| *pn)
                .filter(|pn| Self::page_overlaps_range(*pn, &range))
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|pn| cache.pop(&pn).map(|page| (pn, page)))
                .map(|(pn, page)| self.begin_evicted_page(pn, page, false, false))
                .collect::<Vec<_>>()
        };
        if evictions.is_empty() {
            return Ok((writeback_guard, cache_guard, Vec::new()));
        }
        drop(cache_guard);

        let mut mappings_invalidated = true;
        for eviction in &evictions {
            mappings_invalidated = self.notify_evict_listeners_except(
                eviction.pn,
                eviction.paddr,
                None,
                true,
            ) && mappings_invalidated;
        }

        let cache_guard = self.shared().cache_op.lock();
        let pages = {
            let mut pending = self.shared().pending_evictions.lock();
            evictions
                .iter()
                .map(|eviction| {
                    let entry = pending
                        .remove(&eviction.pn)
                        .expect("length-change eviction token disappeared");
                    assert!(
                        Self::eviction_matches(&entry, *eviction),
                        "length-change eviction token changed generation"
                    );
                    (eviction.pn, entry.page)
                })
                .collect::<Vec<_>>()
        };

        if mappings_invalidated {
            return Ok((writeback_guard, cache_guard, pages));
        }

        let displaced = self.restore_detached_pages_locked(pages);
        drop(cache_guard);
        for eviction in displaced {
            if let Err(err) = self.finish_evicted_page_with_wait(eviction, true) {
                warn!("Failed to finish rollback page displacement: {err:?}");
            }
        }
        Err(VfsError::WouldBlock)
    }

    pub fn set_len(&self, len: u64) -> VfsResult<()> {
        let _append_guard = self.shared().append_lock.write();
        let old_len = self.len()?;
        if old_len == len {
            let _writeback_guard = self.shared().writeback_op.lock();
            let _cache_guard = self.lock_cache_after_draining_pending()?;
            self.inner().entry().as_file()?.set_len(len)?;
            self.inner().set_cached_len_if_present(len);
            return Ok(());
        }

        let affected_start = if old_len > len {
            len / PAGE_SIZE as u64 * PAGE_SIZE as u64
        } else {
            old_len / PAGE_SIZE as u64 * PAGE_SIZE as u64
        };
        let affected_end = if old_len > len {
            u64::MAX
        } else {
            affected_start.saturating_add(PAGE_SIZE as u64)
        };
        // Publish the whole affected range before the first dirty writeback.
        // Faults which sampled the old EOF then retry outside their address-
        // space lock, and ordinary cache users wait behind `writeback_op`.
        let _length_change_guard =
            self.begin_length_change(affected_start..affected_end);
        let file = self.inner().entry().as_file()?;
        let (_writeback_guard, cache_guard, pages) =
            self.lock_pages_for_len_change(affected_start..affected_end)?;

        if let Err(err) = file.set_len(len) {
            let displaced = self.restore_detached_pages_locked(pages);
            drop(cache_guard);
            for eviction in displaced {
                if let Err(finish_err) = self.finish_evicted_page_with_wait(eviction, true) {
                    warn!("Failed to finish set_len rollback displacement: {finish_err:?}");
                }
            }
            return Err(err);
        }

        // The backing operation above is the irreversible commit point.
        // Never follow it with a fallible metadata-cache creation: update the
        // cache only if it still exists, otherwise leave it absent so future
        // EOF validation reads the committed backing length.
        self.inner().set_cached_len_if_present(len);
        let mut restore = Vec::new();
        if old_len > len {
            let partial_pn = (len / PAGE_SIZE as u64) as u32;
            let partial_offset = (len % PAGE_SIZE as u64) as usize;
            for (pn, mut page) in pages {
                if partial_offset != 0 && pn == partial_pn {
                    page.data()[partial_offset..].fill(0);
                    page.dirty = false;
                    restore.push((pn, page));
                }
            }
        } else {
            let zero_start = (old_len % PAGE_SIZE as u64) as usize;
            let zero_end = (len - affected_start).min(PAGE_SIZE as u64) as usize;
            for (pn, mut page) in pages {
                page.data()[zero_start..zero_end].fill(0);
                restore.push((pn, page));
            }
        }

        let displaced = self.restore_detached_pages_locked(restore);
        drop(cache_guard);
        if old_len > len {
            // Two-phase private-map callbacks first revoke WRITE in every
            // address space, then preserve the partial page's private prefix,
            // zero its tail, and unmap pages wholly beyond the new EOF.
            self.notify_length_change_listeners(len..old_len);
        }
        for eviction in displaced {
            if let Err(err) = self.finish_evicted_page_with_wait(eviction, true) {
                // EOF already changed. Keep the displaced page centrally pinned
                // for retry instead of turning a committed truncate into a
                // caller-visible error which might be retried.
                warn!("Deferred set_len page displacement failed: {err:?}");
            }
        }
        Ok(())
    }

    fn page_write_len(file_len: u64, pn: u32) -> usize {
        let page_start = pn as u64 * PAGE_SIZE as u64;
        file_len.saturating_sub(page_start).min(PAGE_SIZE as u64) as usize
    }

    fn writeback_dirty_run(
        file: &FileNode,
        file_len: u64,
        run: &mut [(PageEviction, PendingEviction)],
    ) -> VfsResult<()> {
        if run.is_empty() {
            return Ok(());
        }

        let start = run[0].0.pn as u64 * PAGE_SIZE as u64;
        let mut buf = Vec::new();
        for (eviction, entry) in run.iter_mut() {
            let len = Self::page_write_len(file_len, eviction.pn);
            if len > 0 {
                buf.extend_from_slice(&entry.page.data()[..len]);
            }
        }
        let mut written = 0;
        while written < buf.len() {
            let n = file.write_at(&buf[written..], start + written as u64)?;
            if n == 0 || n > buf.len() - written {
                return Err(VfsError::Io);
            }
            written += n;
        }
        for (_, entry) in run.iter_mut() {
            entry.page.dirty = false;
        }
        Ok(())
    }

    fn writeback_dirty_pages(
        file: &FileNode,
        file_len: u64,
        dirty_pages: &mut Vec<(PageEviction, PendingEviction)>,
    ) -> VfsResult<()> {
        dirty_pages.sort_by_key(|(eviction, _)| eviction.pn);

        let mut start = 0;
        while start < dirty_pages.len() {
            let mut end = start + 1;
            let mut bytes = Self::page_write_len(file_len, dirty_pages[start].0.pn);
            while end < dirty_pages.len() {
                let pn = dirty_pages[end].0.pn;
                let prev_pn = dirty_pages[end - 1].0.pn;
                if pn != prev_pn + 1 {
                    break;
                }
                let len = Self::page_write_len(file_len, pn);
                if bytes > 0 && bytes + len > WRITEBACK_BATCH_BYTES {
                    break;
                }
                bytes += len;
                end += 1;
            }
            Self::writeback_dirty_run(file, file_len, &mut dirty_pages[start..end])?;
            start = end;
        }

        Ok(())
    }

    /// Revokes every writable mapping before performing the final dirty-page
    /// writeback.  Ready pages are still written as contiguous batches; pages
    /// whose listener failed remain pinned in `pending_evictions` for retry.
    fn finish_sync_evictions(&self, evictions: Vec<PageEviction>) -> VfsResult<()> {
        let mut ready = Vec::new();
        let mut blocked = false;
        for eviction in evictions {
            if self.notify_evict_listeners_except(eviction.pn, eviction.paddr, None, true) {
                ready.push(eviction);
            } else {
                blocked = true;
            }
        }

        let displaced = self.finish_evicted_writeback_batch(&ready)?;
        for eviction in displaced {
            self.finish_evicted_page_with_wait(eviction, true)?;
        }

        if blocked {
            Err(VfsError::WouldBlock)
        } else {
            Ok(())
        }
    }

    fn writeback_dirty_pages_in_range_locked(&self, range: &Range<u64>) -> VfsResult<()> {
        if self.in_memory() || range.start >= range.end {
            return Ok(());
        }
        let evictions = {
            // Include dirty pages which an earlier writeback left pending and
            // exclude a new replacement from appearing before the LRU scan.
            let _cache_guard = self.lock_cache_after_draining_pending()?;
            let mut cache = self.shared().page_cache.lock();
            let keys = cache
                .iter()
                .filter(|(pn, page)| Self::page_overlaps_range(**pn, range) && page.dirty)
                .map(|(pn, _)| *pn)
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|pn| cache.pop(&pn).map(|page| (pn, page)))
                .map(|(pn, page)| self.begin_evicted_page(pn, page, false, true))
                .collect::<Vec<_>>()
        };
        self.finish_sync_evictions(evictions)
    }

    fn page_overlaps_range(pn: u32, range: &Range<u64>) -> bool {
        let page_start = u64::from(pn) * PAGE_SIZE as u64;
        page_start < range.end && page_start.saturating_add(PAGE_SIZE as u64) > range.start
    }

    /// Flushes dirty cached data in `range` and returns with both direct-I/O
    /// serialization locks held.  The final dirty recheck is made while
    /// `cache_op` is held, closing the gap in which a cached writer or a new
    /// writable mmap fault could otherwise race the backing-file operation.
    fn lock_direct_read_range(
        &self,
        range: Range<u64>,
    ) -> VfsResult<(MutexGuard<'_, ()>, MutexGuard<'_, ()>)> {
        let writeback_guard = self.shared().writeback_op.lock();
        loop {
            self.writeback_dirty_pages_in_range_locked(&range)?;
            let cache_guard = self.lock_cache_after_draining_pending()?;
            let dirty = self
                .shared()
                .page_cache
                .lock()
                .iter()
                .any(|(pn, page)| Self::page_overlaps_range(*pn, &range) && page.dirty);
            if !dirty {
                return Ok((writeback_guard, cache_guard));
            }
            drop(cache_guard);
        }
    }

    /// Returns with no cached or pending page overlapping `range`, while
    /// holding the same locks across the following direct write.  Clean pages
    /// are invalidated before the backing file is changed, so no stale mmap
    /// translation or post-write dirty-bit clearing window is possible.
    fn lock_direct_write_range(
        &self,
        range: Range<u64>,
    ) -> VfsResult<(MutexGuard<'_, ()>, MutexGuard<'_, ()>)> {
        let writeback_guard = self.shared().writeback_op.lock();
        loop {
            self.writeback_dirty_pages_in_range_locked(&range)?;
            let cache_guard = self.lock_cache_after_draining_pending()?;
            let evictions = {
                let mut cache = self.shared().page_cache.lock();
                if cache
                    .iter()
                    .any(|(pn, page)| Self::page_overlaps_range(*pn, &range) && page.dirty)
                {
                    drop(cache);
                    drop(cache_guard);
                    continue;
                }

                let keys = cache
                    .iter()
                    .map(|(pn, _)| *pn)
                    .filter(|pn| Self::page_overlaps_range(*pn, &range))
                    .collect::<Vec<_>>();
                keys.into_iter()
                    .filter_map(|pn| cache.pop(&pn).map(|page| (pn, page)))
                    .map(|(pn, page)| self.begin_evicted_page(pn, page, false, false))
                    .collect::<Vec<_>>()
            };

            if evictions.is_empty() {
                return Ok((writeback_guard, cache_guard));
            }
            drop(cache_guard);
            for eviction in evictions {
                self.finish_evicted_page_with_wait(eviction, true)?;
            }
        }
    }

    fn discard_cached_pages(&self) {
        // This path intentionally discards cache contents and cannot report an
        // error.  It may help normal replacement tokens make progress, but it
        // must never consume a sync-owned token and hide its writeback status.
        let _ = self.retry_pending_normal_evictions();
        let evictions = {
            let _cache_guard = self.shared().cache_op.lock();
            let mut cache = self.shared().page_cache.lock();
            let mut removed = Vec::new();
            while let Some((pn, page)) = cache.pop_lru() {
                removed.push((pn, page));
            }
            removed
                .into_iter()
                .map(|(pn, mut page)| {
                    page.dirty = false;
                    self.begin_evicted_page(pn, page, false, false)
                })
                .collect::<Vec<_>>()
        };

        for eviction in evictions {
            if let Err(err) = self.finish_evicted_page_with_wait(eviction, true) {
                warn!("Failed to discard cached page: {err:?}");
            }
        }
    }

    fn remove_unused_global_record(&self, discard_metadata: bool) {
        if self.remove_global_record_if_unused() {
            if discard_metadata {
                self.inner.discard_metadata_cache();
            } else {
                self.inner.invalidate_metadata_cache();
            }
        }
    }

    fn prune_inactive_file_caches(retain_key: FileCacheKey) {
        let inactive = {
            let caches = FILE_CACHES.read();
            caches
                .iter()
                .filter(|(key, record)| {
                    **key != retain_key
                        && !record.in_memory
                        && record.active_handles.load(Ordering::Acquire) == 0
                })
                .map(|(key, record)| (*key, Self::from_record(record)))
                .collect::<Vec<_>>()
        };
        let inactive_after_current_drop = inactive.len() + 1;
        if inactive_after_current_drop <= MAX_RETAINED_FILE_CACHE_RECORDS {
            return;
        }

        for (_, cached) in inactive
            .into_iter()
            .take(inactive_after_current_drop - MAX_RETAINED_FILE_CACHE_RECORDS)
        {
            if let Err(err) = cached.drop_cached_pages() {
                warn!("Failed to prune inactive file cache: {err:?}");
                continue;
            }
            if cached.remove_global_record_if_only_snapshot() {
                cached.inner.invalidate_metadata_cache();
            }
        }
    }

    fn writeback_dirty_pages_in_cache(&self) -> VfsResult<()> {
        if self.in_memory() {
            return Ok(());
        }
        let _writeback_guard = self.shared().writeback_op.lock();
        // Serialize detach -> PTE/TLB revocation -> I/O -> restore.  A second
        // fsync waits here and then retries any dirty pending page left by an
        // I/O failure instead of incorrectly reporting success from an empty
        // LRU scan.
        let evictions = {
            let _cache_guard = self.lock_cache_after_draining_pending()?;
            let mut cache = self.shared().page_cache.lock();
            let keys = cache
                .iter()
                .filter(|(_, page)| page.dirty)
                .map(|(pn, _)| *pn)
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|pn| cache.pop(&pn).map(|page| (pn, page)))
                .map(|(pn, page)| self.begin_evicted_page(pn, page, false, true))
                .collect::<Vec<_>>()
        };
        self.finish_sync_evictions(evictions)
    }

    fn drop_cached_pages(&self) -> VfsResult<()> {
        if self.in_memory() {
            return Ok(());
        }
        let _writeback_guard = self.shared().writeback_op.lock();
        let evictions = {
            let _cache_guard = self.lock_cache_after_draining_pending()?;
            let mut cache = self.shared().page_cache.lock();
            let mut removed = Vec::new();
            while let Some((pn, page)) = cache.pop_lru() {
                removed.push((pn, page));
            }
            removed
                .into_iter()
                .map(|(pn, page)| self.begin_evicted_page(pn, page, true, false))
                .collect::<Vec<_>>()
        };
        for eviction in evictions {
            // Explicit cache dropping owns no address-space lock, so listener
            // contention is an internal wait rather than a user-visible error.
            self.finish_evicted_page_with_wait(eviction, true)?;
        }
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
        let previous = self.record.active_handles.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "CachedFile active-handle counter underflow");
        if previous != 1 {
            return;
        }

        if self.record.removed.load(Ordering::Acquire) {
            if !self.in_memory()
                && let Err(err) = self
                    .writeback_dirty_pages_in_cache()
                    .and_then(|_| self.commit_atime())
            {
                warn!("Failed to write back removed file cache: {err:?}");
                return;
            }
            if self.remove_global_record_if_unused() {
                if self.in_memory() {
                    self.discard_cached_pages();
                }
                self.inner.discard_metadata_cache();
            }
            return;
        }

        if !self.tracked {
            return;
        }

        if self.in_memory() {
            return;
        }

        if let Err(err) = self.writeback_dirty_pages_in_cache().and_then(|_| self.commit_atime()) {
            if err.canonicalize() == VfsError::StorageFull {
                self.discard_cached_pages();
                self.remove_unused_global_record(true);
                return;
            }
            warn!("Failed to write back unused file cache: {err:?}");
            return;
        }

        Self::prune_inactive_file_caches(self.record.key);
    }
}

pub fn sync_global_file_caches() -> VfsResult<()> {
    let cached_files = {
        let caches = FILE_CACHES.read();
        caches
            .values()
            .map(CachedFile::from_record)
            .collect::<Vec<_>>()
    };
    for cached in cached_files {
        if cached.in_memory() {
            continue;
        }
        cached.sync(false)?;
    }
    flush_inode_metadata_caches(false)?;
    Ok(())
}

pub fn drop_global_file_caches() -> VfsResult<()> {
    let cached_files = {
        let caches = FILE_CACHES.read();
        caches
            .values()
            .map(CachedFile::from_record)
            .collect::<Vec<_>>()
    };
    for cached in cached_files {
        if cached.in_memory() {
            continue;
        }
        cached.drop_cached_pages()?;
    }
    Ok(())
}

pub fn drop_file_caches_by_device(device: u64) -> VfsResult<()> {
    let cached_files = {
        let caches = FILE_CACHES.read();
        caches
            .values()
            .filter(|record| record.location.mountpoint().device() == device)
            .map(CachedFile::from_record)
            .collect::<Vec<_>>()
    };

    for cached in cached_files {
        cached.drop_cached_pages()?;
        if cached.remove_global_record_if_only_snapshot() {
            cached.inner.invalidate_metadata_cache();
        }
    }
    Ok(())
}

pub fn invalidate_file_cache(location: &Location) -> VfsResult<()> {
    let key = CachedFile::file_cache_key(location);
    let Some(cached) = CachedFile::get_existing(key) else {
        location.invalidate_metadata_cache();
        return Ok(());
    };
    debug_assert_eq!(cached.record.key, key);
    if cached.in_memory() {
        // tmpfs has no backing device: this canonical record owns the file
        // contents even when no descriptor currently holds a CachedFile.
        // Invalidating metadata must never retire its pages while it remains
        // linked in the namespace.
    } else {
        cached.drop_cached_pages()?;
        let _ = cached.remove_global_record_if_only_snapshot();
    }
    location.invalidate_metadata_cache();
    Ok(())
}

pub fn invalidate_removed_file_cache(location: &Location) -> VfsResult<()> {
    let key = CachedFile::file_cache_key(location);
    let Some(cached) = CachedFile::get_existing(key) else {
        // Preparation precedes the namespace mutation and must remain
        // rollback-safe if unlink/rename later fails.
        location.invalidate_metadata_cache();
        return Ok(());
    };

    debug_assert_eq!(cached.record.key, key);
    if !cached.in_memory() {
        cached.drop_cached_pages()?;
    }
    // This is the prepare half and runs before the namespace mutation.  Keep
    // the canonical record intact until unlink/rename has actually succeeded;
    // otherwise an open in the gap, or a failed unlink, could observe an empty
    // replacement record (catastrophic for tmpfs, whose pages are the data).
    location.invalidate_metadata_cache();
    Ok(())
}

/// Commits cache retirement after unlink or replacement has succeeded.
pub fn finalize_removed_file_cache(location: &Location) {
    let key = CachedFile::file_cache_key(location);
    let Some(cached) = CachedFile::get_existing(key) else {
        location.discard_metadata_cache();
        return;
    };

    debug_assert_eq!(cached.record.key, key);
    cached.record.removed.store(true, Ordering::Release);
    if cached.remove_global_record_if_only_snapshot() {
        if cached.in_memory() {
            cached.discard_cached_pages();
        }
        location.discard_metadata_cache();
    } else {
        // Existing open files and mmap backends keep the record and its pages.
        // Their last CachedFile::drop observes `removed` and performs final
        // retirement, avoiding both split lock domains and tmpfs leaks.
        location.invalidate_metadata_cache();
    }
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

/// Owns one file-length callback for the lifetime of a private mapping.
/// Dropping the last mapping clone unregisters it before releasing the cached
/// inode handle.
pub struct FileLengthChangeListener {
    cached: CachedFile,
    handle: usize,
}

impl Drop for FileLengthChangeListener {
    fn drop(&mut self) {
        self.cached.remove_length_change_listener(self.handle);
    }
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
            Self::Cached(cached) => cached.read_at(dst, offset),
            Self::Direct(loc) => {
                let start = offset;
                let len = dst.remaining_mut();
                // Direct and cached handles must join the same inode lock
                // domain even when this is the first operation to create it.
                // A lookup-only fast path would race a concurrent first mmap
                // or cached open between the lookup and backing-file I/O.
                let cached = CachedFile::get_or_create(loc.clone());
                let _direct_io_guards = if len > 0 {
                    Some(cached.lock_direct_read_range(
                        start..start.saturating_add(len as u64),
                    )?)
                } else {
                    None
                };
                dst.read_from(&mut axio::read_fn(|buf| {
                    loc.entry().as_file()?.read_at(buf, offset).inspect(|read| {
                        offset += *read as u64;
                    })
                }))
            }
        }
    }

    /// Coherent read which never replaces a cached page and therefore never
    /// invokes an mmap eviction listener.
    pub fn read_at_no_evict(
        &self,
        dst: impl Write + IoBufMut,
        offset: u64,
    ) -> VfsResult<usize> {
        match self {
            Self::Cached(cached) => cached.read_at_no_evict(dst, offset),
            Self::Direct(loc) => {
                // Join the inode cache domain before the lookup.  Otherwise a
                // concurrent first cached writer could create a dirty page
                // after a `get_existing` miss and before the backing read.
                CachedFile::get_or_create(loc.clone()).read_at_no_evict(dst, offset)
            }
        }
    }

    /// Returns true while truncate is changing the page containing `offset`.
    /// Fault code uses this as the prepare/commit generation barrier.
    pub fn length_change_in_progress(&self, offset: u64) -> bool {
        let cached = match self {
            Self::Cached(cached) => cached.clone(),
            Self::Direct(location) => CachedFile::get_or_create(location.clone()),
        };
        let pn = (offset / PAGE_SIZE as u64) as u32;
        cached.length_change_blocks_page(pn)
    }

    /// Returns a stable file-length generation when the page containing
    /// `offset` is not being changed.  A fault must compare this value again
    /// after reading and before publishing its PTE.
    pub fn length_change_generation(&self, offset: u64) -> Option<usize> {
        let cached = match self {
            Self::Cached(cached) => cached.clone(),
            Self::Direct(location) => CachedFile::get_or_create(location.clone()),
        };
        let pn = (offset / PAGE_SIZE as u64) as u32;
        cached.length_generation_for_page(pn)
    }

    /// Registers an observer for complete file pages invalidated by shrink.
    pub fn add_length_change_listener<F>(&self, listener: F) -> FileLengthChangeListener
    where
        F: Fn(Range<u64>, FileLengthChangePhase) + Send + Sync + 'static,
    {
        let cached = match self {
            Self::Cached(cached) => cached.clone(),
            Self::Direct(location) => CachedFile::get_or_create(location.clone()),
        };
        let handle = cached.add_length_change_listener(listener);
        FileLengthChangeListener { cached, handle }
    }

    pub fn write_at(&self, mut src: impl Read + IoBuf, mut offset: u64) -> VfsResult<usize> {
        match self {
            Self::Cached(cached) => cached.write_at(src, offset),
            Self::Direct(loc) => {
                let start = offset;
                let len = src.remaining();
                let cached = CachedFile::get_or_create(loc.clone());
                let _append_guard = cached.shared().append_lock.read();
                let _direct_io_guards = if len > 0 {
                    Some(cached.lock_direct_write_range(
                        start..start.saturating_add(len as u64),
                    )?)
                } else {
                    None
                };
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
                let cached = CachedFile::get_or_create(loc.clone());
                let _append_guard = cached.shared().append_lock.write();
                let _direct_io_guards = cached.lock_direct_write_range(0..u64::MAX)?;
                loc.entry().as_file()?.sync(true)?;
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
                if start.is_some() {
                    loc.set_cached_len_if_present(end);
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
            // Truncate must publish the same length-change transaction and
            // notify MAP_PRIVATE observers regardless of the descriptor's
            // O_DIRECT mode.
            Self::Direct(loc) => CachedFile::get_or_create(loc.clone()).set_len(len),
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
