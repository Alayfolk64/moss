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
    sync::atomic::{AtomicUsize, Ordering},
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
        }
    }
}

type FileCacheKey = (usize, u64);

struct CachedFileRecord {
    key: FileCacheKey,
    location: Location,
    shared: Arc<CachedFileShared>,
    in_memory: bool,
}

static FILE_CACHES: spin::Lazy<RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>>> =
    spin::Lazy::new(|| RwLock::new(BTreeMap::new()));

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

    fn record_from_global_cache(location: &Location, in_memory: bool) -> Arc<CachedFileRecord> {
        let key = Self::file_cache_key(location);
        let mut caches = FILE_CACHES.write();
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

        Self {
            inner: location,
            record,
        }
    }

    fn get_existing(location: &Location) -> Option<Self> {
        let key = Self::file_cache_key(location);
        FILE_CACHES.read().get(&key).cloned().map(|record| Self {
            inner: location.clone(),
            record,
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

    fn shared(&self) -> &CachedFileShared {
        self.record.shared.as_ref()
    }

    fn page_range(range: Range<u64>) -> Range<u32> {
        let start = (range.start / PAGE_SIZE as u64) as u32;
        let end = range.end.div_ceil(PAGE_SIZE as u64) as u32;
        start..end
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

    fn from_record(record: &Arc<CachedFileRecord>) -> Self {
        Self {
            inner: record.location.clone(),
            record: record.clone(),
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
        self.shared()
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
                let pending_blocks_access = wait_for_pending
                    && self
                        .shared()
                        .pending_evictions
                        .lock()
                        .get(&pn)
                        .is_some_and(|entry| !entry.reusable);
                if pending_blocks_access {
                    // Do not wait while holding `cache_op`: the writeback owner
                    // needs it to restore or retire this exact token.  Once the
                    // current owner releases `writeback_op`, retry any lifecycle
                    // which ended with a recoverable listener failure.
                    drop(cache_guard);
                    let _writeback_guard = self.shared().writeback_op.lock();
                    self.retry_pending_evictions()?;
                    continue;
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
                    Ok(()) | Err(VfsError::WouldBlock) => {
                        // A busy address space leaves this reusable replacement in
                        // `pending_evictions`.  The frame is still pinned and later
                        // operations retry it, so ordinary file I/O must not fail
                        // merely because a mapping lock could not be acquired
                        // without creating an ABBA cycle.
                        Ok(value)
                    }
                    Err(err) => Err(err),
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

            let mut guard = self.shared().page_cache.lock();
            if let Some(page) = guard.get_mut(&pn) {
                dst.write(&page.data()[range_start..range_end])?;
            } else {
                dst.write(&ZERO_PAGE[range_start..range_end])?;
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
                self.set_cached_len(end, false)?;
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

    pub fn set_len(&self, len: u64) -> VfsResult<()> {
        let _writeback_guard = self.shared().writeback_op.lock();
        let evictions = {
            // A pending dirty page may be absent from the LRU after an earlier
            // I/O failure.  Drain under the cache serialization point before
            // changing EOF so no concurrent replacement can appear between
            // the pending scan and this length update.
            let _cache_guard = self.lock_cache_after_draining_pending()?;
            let file = self.inner().entry().as_file()?;
            let old_len = self.len()?;
            file.set_len(len)?;
            self.set_cached_len(len, false)?;

            let old_last_page = (old_len / PAGE_SIZE as u64) as u32;
            let removed = if old_len < len {
                let page_start = old_last_page as u64 * PAGE_SIZE as u64;
                let old_page_offset = (old_len - page_start) as usize;
                let new_page_offset = (len - page_start).min(PAGE_SIZE as u64) as usize;
                let mut guard = self.shared().page_cache.lock();
                if let Some(page) = guard.get_mut(&old_last_page) {
                    page.data()[old_page_offset..new_page_offset].fill(0);
                }
                Vec::new()
            } else if old_len > len {
                // For truncating, remove all pages beyond the new length.
                let first_invalid_page = len.div_ceil(PAGE_SIZE as u64) as u32;
                let mut guard = self.shared().page_cache.lock();
                let keys = guard
                    .iter()
                    .map(|(k, _)| *k)
                    .filter(|it| *it >= first_invalid_page)
                    .collect::<Vec<_>>();
                let mut removed = Vec::new();
                for pn in keys {
                    if let Some(mut page) = guard.pop(&pn) {
                        // Don't write back pages since they're discarded.
                        page.dirty = false;
                        removed.push((pn, page));
                    }
                }
                removed
            } else {
                Vec::new()
            };
            removed
                .into_iter()
                .map(|(pn, page)| self.begin_evicted_page(pn, page, false, false))
                .collect::<Vec<_>>()
        };
        for eviction in evictions {
            // Truncate owns `writeback_op` and no address-space lock.  Wait for
            // transient mmap activity so EOF cannot change while a detached,
            // non-reusable page is left behind with `WouldBlock`.
            self.finish_evicted_page_with_wait(eviction, true)?;
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

    fn writeback_dirty_pages_in_range(&self, range: Range<u64>) -> VfsResult<()> {
        if self.in_memory() || range.start >= range.end {
            return Ok(());
        }
        let _writeback_guard = self.shared().writeback_op.lock();
        let page_range = Self::page_range(range);
        let evictions = {
            // Include dirty pages which an earlier writeback left pending and
            // exclude a new replacement from appearing before the LRU scan.
            let _cache_guard = self.lock_cache_after_draining_pending()?;
            let mut cache = self.shared().page_cache.lock();
            let keys = cache
                .iter()
                .filter(|(pn, page)| {
                    **pn >= page_range.start && **pn < page_range.end && page.dirty
                })
                .map(|(pn, _)| *pn)
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|pn| cache.pop(&pn).map(|page| (pn, page)))
                .map(|(pn, page)| self.begin_evicted_page(pn, page, false, true))
                .collect::<Vec<_>>()
        };
        self.finish_sync_evictions(evictions)
    }

    fn invalidate_clean_pages_in_range(&self, range: Range<u64>) {
        if self.in_memory() || range.start >= range.end {
            return;
        }
        let page_range = Self::page_range(range);
        let evictions = {
            let _cache_guard = self.shared().cache_op.lock();
            let mut cache = self.shared().page_cache.lock();
            let mut removed = Vec::new();
            let keys = cache
                .iter()
                .map(|(pn, _)| *pn)
                .filter(|pn| *pn >= page_range.start && *pn < page_range.end)
                .collect::<Vec<_>>();
            for pn in keys {
                if let Some(page) = cache.pop(&pn) {
                    removed.push((pn, page));
                }
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
                warn!("Failed to invalidate cached page: {err:?}");
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
        let mut caches = FILE_CACHES.write();
        if Arc::strong_count(&self.record) == 2
            && caches
                .get(&self.record.key)
                .is_some_and(|record| Arc::ptr_eq(record, &self.record))
        {
            caches.remove(&self.record.key);
            self.inner.user_data().remove::<FileUserData>();
            if discard_metadata {
                self.inner.discard_metadata_cache();
            } else {
                self.inner.invalidate_metadata_cache();
            }
        }
    }

    fn prune_inactive_file_caches(retain_key: FileCacheKey) {
        let inactive = FILE_CACHES
            .read()
            .iter()
            .filter(|(key, record)| {
                **key != retain_key && !record.in_memory && Arc::strong_count(record) == 1
            })
            .map(|(key, record)| (*key, record.clone()))
            .collect::<Vec<_>>();
        let inactive_after_current_drop = inactive.len() + 1;
        if inactive_after_current_drop <= MAX_RETAINED_FILE_CACHE_RECORDS {
            return;
        }

        for (key, record) in inactive
            .into_iter()
            .take(inactive_after_current_drop - MAX_RETAINED_FILE_CACHE_RECORDS)
        {
            let cached = Self::from_record(&record);
            if let Err(err) = cached.drop_cached_pages() {
                warn!("Failed to prune inactive file cache: {err:?}");
                continue;
            }
            drop(cached);

            let mut caches = FILE_CACHES.write();
            if Arc::strong_count(&record) == 2
                && caches
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(current, &record))
            {
                caches.remove(&key);
                record.location.user_data().remove::<FileUserData>();
                record.location.invalidate_metadata_cache();
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
        if self.in_memory() || Arc::strong_count(&self.record) != 2 {
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
    let records = FILE_CACHES.read().values().cloned().collect::<Vec<_>>();
    for record in records {
        if record.in_memory {
            continue;
        }
        CachedFile::from_record(&record).sync(false)?;
    }
    flush_inode_metadata_caches(false)?;
    Ok(())
}

pub fn drop_global_file_caches() -> VfsResult<()> {
    let records = FILE_CACHES.read().values().cloned().collect::<Vec<_>>();
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
        .read()
        .iter()
        .filter(|(_, record)| record.location.mountpoint().device() == device)
        .map(|(key, record)| (*key, record.clone()))
        .collect::<Vec<_>>();

    for (_, record) in &records {
        CachedFile::from_record(record).drop_cached_pages()?;
    }

    let mut caches = FILE_CACHES.write();
    for (key, record) in records {
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
    let Some(record) = FILE_CACHES.read().get(&key).cloned() else {
        location.invalidate_metadata_cache();
        return Ok(());
    };
    debug_assert_eq!(record.key, key);
    if record.in_memory {
        FILE_CACHES.write().remove(&key);
        location.user_data().remove::<FileUserData>();
        location.invalidate_metadata_cache();
        return Ok(());
    }
    CachedFile::from_record(&record).drop_cached_pages()?;
    FILE_CACHES.write().remove(&key);
    location.user_data().remove::<FileUserData>();
    location.invalidate_metadata_cache();
    Ok(())
}

pub fn invalidate_removed_file_cache(location: &Location) -> VfsResult<()> {
    let key = CachedFile::file_cache_key(location);
    let Some(record) = FILE_CACHES.read().get(&key).cloned() else {
        location.discard_metadata_cache();
        return Ok(());
    };

    debug_assert_eq!(record.key, key);
    if Arc::strong_count(&record) == 2 {
        CachedFile::from_record(&record).discard_cached_pages();
        FILE_CACHES.write().remove(&key);
        location.user_data().remove::<FileUserData>();
        location.discard_metadata_cache();
        return Ok(());
    }

    if record.in_memory {
        FILE_CACHES.write().remove(&key);
        location.user_data().remove::<FileUserData>();
        location.invalidate_metadata_cache();
        return Ok(());
    }

    CachedFile::from_record(&record).drop_cached_pages()?;
    FILE_CACHES.write().remove(&key);
    location.user_data().remove::<FileUserData>();
    location.invalidate_metadata_cache();
    Ok(())
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
            Self::Cached(cached) => cached.read_at(dst, offset),
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

    /// Coherent read which never replaces a cached page and therefore never
    /// invokes an mmap eviction listener.
    pub fn read_at_no_evict(
        &self,
        mut dst: impl Write + IoBufMut,
        mut offset: u64,
    ) -> VfsResult<usize> {
        match self {
            Self::Cached(cached) => cached.read_at_no_evict(dst, offset),
            Self::Direct(loc) => {
                if let Some(cached) = CachedFile::get_existing(loc) {
                    return cached.read_at_no_evict(dst, offset);
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
                let cached = CachedFile::get_existing(loc);
                if let Some(cached) = cached.as_ref() {
                    cached.sync(true)?;
                }
                loc.entry().as_file()?.set_len(len)?;
                loc.set_cached_len(len, false)?;
                if let Some(cached) = cached {
                    cached.drop_cached_pages()?;
                }
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
