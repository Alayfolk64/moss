use alloc::{
    boxed::Box,
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
#[cfg(feature = "times")]
use core::sync::atomic::{AtomicU8, Ordering};
use core::{num::NonZeroUsize, ops::Range, task::Context};

use axalloc::{UsageKind, global_allocator};
use axfs_ng_vfs::{
    FileNode, Location, NodeFlags, NodePermission, NodeType, VfsError, VfsResult, path::Path,
};
use axhal::mem::{PhysAddr, VirtAddr, virt_to_phys};
use axio::{SeekFrom, prelude::*};
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
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
                loc.metadata()?.node_type,
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
const DEFAULT_FILE_CACHE_PAGES: usize = 256;
const MAX_READAHEAD_BYTES: usize = DEFAULT_FILE_CACHE_PAGES * PAGE_SIZE;
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
    listener: Box<dyn Fn(u32, &PageCache) + Send + Sync>,
    link: LinkedListAtomicLink,
}

intrusive_adapter!(EvictListenerAdapter = Box<EvictListener>: EvictListener { link: LinkedListAtomicLink });

struct CachedFileShared {
    page_cache: Mutex<LruCache<u32, PageCache>>,
    evict_listeners: Mutex<LinkedList<EvictListenerAdapter>>,
}

impl CachedFileShared {
    pub fn new() -> Self {
        Self {
            page_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(DEFAULT_FILE_CACHE_PAGES).unwrap(),
            )),
            evict_listeners: Mutex::new(LinkedList::default()),
        }
    }

    pub fn new_unbounded() -> Self {
        Self {
            page_cache: Mutex::new(LruCache::unbounded()),
            evict_listeners: Mutex::new(LinkedList::default()),
        }
    }
}

type FileCacheKey = (usize, u64);

struct CachedFileRecord {
    location: Location,
    shared: Arc<CachedFileShared>,
    in_memory: bool,
}

static FILE_CACHES: spin::Lazy<RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>>> =
    spin::Lazy::new(|| RwLock::new(BTreeMap::new()));

pub struct CachedFile {
    inner: Location,
    shared: Arc<CachedFileShared>,
    in_memory: bool,
    /// Only one thread can append to the file at a time, while multiple writers
    /// are permitted.
    append_lock: RwLock<()>,
}

impl Clone for CachedFile {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            shared: self.shared.clone(),
            in_memory: self.in_memory,
            append_lock: RwLock::new(()),
        }
    }
}

enum FileUserData {
    Weak(Weak<CachedFileShared>),
    Strong(Arc<CachedFileShared>),
}

impl FileUserData {
    pub fn get(&self) -> Option<Arc<CachedFileShared>> {
        match self {
            FileUserData::Weak(weak) => weak.upgrade(),
            FileUserData::Strong(strong) => Some(strong.clone()),
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
        let shared = if let Some(shared) = guard.get::<FileUserData>().and_then(|it| it.get()) {
            shared
        } else {
            let record = Self::record_from_global_cache(&location, in_memory);
            let shared = record.shared.clone();
            let user_data = if in_memory {
                FileUserData::Strong(shared.clone())
            } else {
                FileUserData::Weak(Arc::downgrade(&shared))
            };
            guard.insert(user_data);
            shared
        };
        drop(guard);

        Self {
            inner: location,
            shared,
            in_memory,
            append_lock: RwLock::new(()),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub fn in_memory(&self) -> bool {
        self.in_memory
    }

    fn from_record(record: &CachedFileRecord) -> Self {
        Self {
            inner: record.location.clone(),
            shared: record.shared.clone(),
            in_memory: record.in_memory,
            append_lock: RwLock::new(()),
        }
    }

    pub fn add_evict_listener<F>(&self, listener: F) -> usize
    where
        F: Fn(u32, &PageCache) + Send + Sync + 'static,
    {
        let pointer = Box::new(EvictListener {
            listener: Box::new(listener),
            link: LinkedListAtomicLink::new(),
        });
        let handle = pointer.as_ref() as *const EvictListener as usize;
        self.shared.evict_listeners.lock().push_back(pointer);
        handle
    }

    pub unsafe fn remove_evict_listener(&self, handle: usize) {
        let mut guard = self.shared.evict_listeners.lock();
        let mut cursor = unsafe { guard.cursor_mut_from_ptr(handle as *const EvictListener) };
        cursor.remove();
    }

    fn notify_evict_listeners(&self, pn: u32, page: &PageCache) {
        for listener in self.shared.evict_listeners.lock().iter() {
            (listener.listener)(pn, page);
        }
    }

    fn writeback_page(file: &FileNode, pn: u32, page: &mut PageCache) -> VfsResult<()> {
        if page.dirty {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let len = (file.len()?.saturating_sub(page_start)).min(PAGE_SIZE as u64) as usize;
            if len > 0 {
                file.write_at(&page.data()[..len], page_start)?;
            }
            page.dirty = false;
        }
        Ok(())
    }

    fn evict_cache(&self, file: &FileNode, pn: u32, page: &mut PageCache) -> VfsResult<()> {
        self.notify_evict_listeners(pn, page);
        Self::writeback_page(file, pn, page)
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
        let mut evicted = None;
        if cache.len() == cache.cap().get() {
            // Cache is full, remove the least recently used page
            if let Some((pn, mut page)) = cache.pop_lru() {
                self.evict_cache(file, pn, &mut page)?;
                evicted = Some((pn, page));
            }
        }

        // Page not in cache, read it
        let mut page = PageCache::new()?;
        page.data().fill(0);
        if !self.in_memory {
            file.read_at(page.data(), pn as u64 * PAGE_SIZE as u64)?;
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
        let mut evicted = None;
        if cache.len() == cache.cap().get() {
            if let Some((pn, mut page)) = cache.pop_lru() {
                self.evict_cache(file, pn, &mut page)?;
                evicted = Some((pn, page));
            }
        }

        let mut page = PageCache::new()?;
        page.data().fill(0);
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
        f(self.shared.page_cache.lock().get_mut(&pn))
    }

    pub fn with_page_or_insert<R>(
        &self,
        pn: u32,
        f: impl FnOnce(&mut PageCache, Option<(u32, PageCache)>) -> VfsResult<R>,
    ) -> VfsResult<R> {
        let mut guard = self.shared.page_cache.lock();
        let (page, evicted) = self.page_or_insert(self.inner.entry().as_file()?, &mut guard, pn)?;
        f(page, evicted)
    }

    fn with_pages<T>(
        &self,
        range: Range<u64>,
        page_initial: impl FnOnce(&FileNode) -> VfsResult<T>,
        mut page_each: impl FnMut(T, &mut PageCache, Range<usize>) -> VfsResult<T>,
    ) -> VfsResult<T> {
        let file = self.inner.entry().as_file()?;
        let mut initial = page_initial(file)?;
        let start_page = (range.start / PAGE_SIZE as u64) as u32;
        let end_page = range.end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (range.start % PAGE_SIZE as u64) as usize;
        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;

            let mut guard = self.shared.page_cache.lock();
            let page = self.page_or_insert(file, &mut guard, pn)?.0;

            initial = page_each(
                initial,
                page,
                page_offset..(range.end - page_start).min(PAGE_SIZE as u64) as usize,
            )?;
            page_offset = 0;
        }

        Ok(initial)
    }

    pub fn read_at(&self, mut dst: impl Write + IoBufMut, offset: u64) -> VfsResult<usize> {
        let len = self.inner.len()?;
        let end = (offset + dst.remaining_mut() as u64).min(len);
        if end <= offset {
            return Ok(0);
        }
        if self.in_memory {
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

    pub fn readahead(&self, offset: u64, count: usize) -> VfsResult<()> {
        if count == 0 || self.in_memory {
            return Ok(());
        }

        let len = self.inner.len()?;
        if offset >= len {
            return Ok(());
        }
        let count = count.min(MAX_READAHEAD_BYTES);
        let end = offset.saturating_add(count as u64).min(len);
        let file = self.inner.entry().as_file()?;
        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;

        for pn in start_page..end_page {
            let mut guard = self.shared.page_cache.lock();
            self.page_or_insert(file, &mut guard, pn)?;
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

            let mut guard = self.shared.page_cache.lock();
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
        if self.in_memory {
            return self.write_at_in_memory(buf, offset, end);
        }

        let file = self.inner.entry().as_file()?;
        let old_len = file.len()?;
        if end > old_len {
            file.set_len(end)?;
        }

        let start_page = (offset / PAGE_SIZE as u64) as u32;
        let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;
        let mut page_offset = (offset % PAGE_SIZE as u64) as usize;
        let mut written = 0;

        for pn in start_page..end_page {
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let range = page_offset..(end - page_start).min(PAGE_SIZE as u64) as usize;
            let len = range.end - range.start;

            let mut guard = self.shared.page_cache.lock();
            let page = if Self::can_zero_fill_write_page(old_len, page_start, &range) {
                self.page_or_insert_zeroed(file, &mut guard, pn)?.0
            } else {
                self.page_or_insert(file, &mut guard, pn)?.0
            };
            buf.read(&mut page.data()[range.start..range.end])?;
            page.dirty = true;
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
        let file = self.inner.entry().as_file()?;
        if end > file.len()? {
            file.set_len(end)?;
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

            let mut guard = self.shared.page_cache.lock();
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
        let _guard = self.append_lock.read();
        self.write_at_locked(buf, offset)
    }

    pub fn append(&self, buf: impl Read + IoBuf) -> VfsResult<(usize, u64)> {
        let _guard = self.append_lock.write();
        let file = self.inner.entry().as_file()?;
        let len = file.len()?;
        self.write_at_locked(buf, len)
            .map(|written| (written, len + written as u64))
    }

    pub fn set_len(&self, len: u64) -> VfsResult<()> {
        let file = self.inner.entry().as_file()?;
        let old_len = file.len()?;
        file.set_len(len)?;

        let old_last_page = (old_len / PAGE_SIZE as u64) as u32;
        if old_len < len {
            let mut guard = self.shared.page_cache.lock();
            if let Some(page) = guard.get_mut(&old_last_page) {
                let page_start = old_last_page as u64 * PAGE_SIZE as u64;
                let old_page_offset = (old_len - page_start) as usize;
                let new_page_offset = (len - page_start).min(PAGE_SIZE as u64) as usize;
                page.data()[old_page_offset..new_page_offset].fill(0);
            }
        } else if old_len > len {
            // For truncating, we need to remove all pages that are beyond the
            // new length
            // TODO(mivik): can this be more efficient?
            let mut guard = self.shared.page_cache.lock();
            let first_invalid_page = len.div_ceil(PAGE_SIZE as u64) as u32;
            let keys = guard
                .iter()
                .map(|(k, _)| *k)
                .filter(|it| *it >= first_invalid_page)
                .collect::<Vec<_>>();
            for pn in keys {
                if let Some(mut page) = guard.pop(&pn)
                    && !self.in_memory
                {
                    // Don't write back pages since they're discarded
                    page.dirty = false;
                    self.evict_cache(file, pn, &mut page)?;
                }
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
        run: &mut [(u32, PageCache)],
    ) -> VfsResult<()> {
        if run.is_empty() {
            return Ok(());
        }

        let start = run[0].0 as u64 * PAGE_SIZE as u64;
        let mut buf = Vec::new();
        for (pn, page) in run.iter_mut() {
            let len = Self::page_write_len(file_len, *pn);
            if len > 0 {
                buf.extend_from_slice(&page.data()[..len]);
            }
        }
        if !buf.is_empty() {
            file.write_at(&buf, start)?;
        }
        for (_, page) in run.iter_mut() {
            page.dirty = false;
        }
        Ok(())
    }

    fn writeback_dirty_pages(
        file: &FileNode,
        dirty_pages: &mut Vec<(u32, PageCache)>,
    ) -> VfsResult<()> {
        dirty_pages.sort_by_key(|(pn, _)| *pn);
        let file_len = file.len()?;

        let mut start = 0;
        while start < dirty_pages.len() {
            let mut end = start + 1;
            let mut bytes = Self::page_write_len(file_len, dirty_pages[start].0);
            while end < dirty_pages.len() {
                let pn = dirty_pages[end].0;
                let prev_pn = dirty_pages[end - 1].0;
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

    fn writeback_dirty_pages_in_cache(&self) -> VfsResult<()> {
        if self.in_memory {
            return Ok(());
        }
        let file = self.inner.entry().as_file()?;
        let mut guard = self.shared.page_cache.lock();
        let dirty_page_numbers = guard
            .iter()
            .filter_map(|(pn, page)| page.dirty.then_some(*pn))
            .collect::<Vec<_>>();
        let mut dirty_pages = Vec::new();
        for pn in dirty_page_numbers {
            if let Some(page) = guard.pop(&pn)
                && page.dirty
            {
                dirty_pages.push((pn, page));
            }
        }
        let result = Self::writeback_dirty_pages(file, &mut dirty_pages);
        for (pn, page) in dirty_pages {
            guard.put(pn, page);
        }
        result
    }

    fn drop_cached_pages(&self) -> VfsResult<()> {
        if self.in_memory {
            return Ok(());
        }
        let file = self.inner.entry().as_file()?;
        let mut dirty_pages = Vec::new();
        let mut guard = self.shared.page_cache.lock();
        while let Some((pn, mut page)) = guard.pop_lru() {
            self.notify_evict_listeners(pn, &page);
            if page.dirty {
                dirty_pages.push((pn, page));
            }
        }
        drop(guard);
        let result = Self::writeback_dirty_pages(file, &mut dirty_pages);
        if result.is_err() {
            let mut guard = self.shared.page_cache.lock();
            for (pn, page) in dirty_pages {
                guard.put(pn, page);
            }
        }
        result
    }

    pub fn sync(&self, data_only: bool) -> VfsResult<()> {
        if self.in_memory {
            return Ok(());
        }
        self.writeback_dirty_pages_in_cache()?;
        let file = self.inner.entry().as_file()?;
        file.sync(data_only)?;
        Ok(())
    }

    pub fn location(&self) -> &Location {
        &self.inner
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

pub fn invalidate_file_cache(location: &Location) -> VfsResult<()> {
    let key = CachedFile::file_cache_key(location);
    let Some(record) = FILE_CACHES.write().remove(&key) else {
        return Ok(());
    };
    if record.in_memory {
        return Ok(());
    }
    CachedFile::from_record(&record).drop_cached_pages()
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
            Self::Direct(loc) => dst.read_from(&mut axio::read_fn(|buf| {
                loc.entry().as_file()?.read_at(buf, offset).inspect(|read| {
                    offset += *read as u64;
                })
            })),
        }
    }

    pub fn write_at(&self, mut src: impl Read + IoBuf, mut offset: u64) -> VfsResult<usize> {
        match self {
            Self::Cached(cached) => cached.write_at(src, offset),
            Self::Direct(loc) => src.write_to(&mut axio::write_fn(|buf| {
                loc.entry()
                    .as_file()?
                    .write_at(buf, offset)
                    .inspect(|written| {
                        offset += *written as u64;
                    })
            })),
        }
    }

    pub fn readahead(&self, offset: u64, count: usize) -> VfsResult<()> {
        match self {
            Self::Cached(cached) => cached.readahead(offset, count),
            Self::Direct(_) => Ok(()),
        }
    }

    pub fn append(&self, mut src: impl Read + IoBuf) -> VfsResult<(usize, u64)> {
        match self {
            Self::Cached(cached) => cached.append(src),
            Self::Direct(loc) => {
                let mut end = 0;
                src.write_to(&mut axio::write_fn(|buf| {
                    loc.entry().as_file()?.append(buf).map(|(n, offset)| {
                        end = offset;
                        n
                    })
                }))
                .map(|n| (n, end))
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
            Self::Direct(loc) => loc.entry().as_file()?.sync(data_only),
        }
    }

    pub fn set_len(&self, len: u64) -> VfsResult<()> {
        match self {
            Self::Cached(cached) => cached.set_len(len),
            Self::Direct(loc) => loc.entry().as_file()?.set_len(len),
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
                inner.location().len().unwrap_or_default()
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

    /// Reads a number of bytes starting from a given offset.
    pub fn read_at(&self, dst: impl Write + IoBufMut, offset: u64) -> VfsResult<usize> {
        self.access(FileFlags::READ)?.read_at(dst, offset)
    }

    /// Writes a number of bytes starting from a given offset.
    pub fn write_at(&self, src: impl Read + IoBuf, offset: u64) -> VfsResult<usize> {
        self.access(FileFlags::WRITE)?.write_at(src, offset)
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
        #[cfg(feature = "times")]
        {
            self.access_flags.fetch_or(1, Ordering::AcqRel);
        }
        if let Some(pos) = self.position.as_ref() {
            let mut pos = pos.lock();
            self.read_at(dst, *pos).inspect(|n| {
                *pos += *n as u64;
            })
        } else {
            self.read_at(dst, 0)
        }
    }

    pub fn write(&self, src: impl Read + IoBuf) -> axio::Result<usize> {
        #[cfg(feature = "times")]
        {
            self.access_flags.fetch_or(3, Ordering::AcqRel);
        }
        if let Some(pos) = self.position.as_ref() {
            let mut pos = pos.lock();
            if let Ok(f) = self.access(FileFlags::APPEND) {
                f.append(src).map(|(written, new_size)| {
                    *pos = new_size;
                    written
                })
            } else {
                self.write_at(src, *pos).inspect(|n| {
                    *pos += *n as u64;
                })
            }
        } else {
            self.write_at(src, 0)
        }
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
                    let size = self.access(FileFlags::empty())?.location().len()?;
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
        let flags = self.access_flags.load(Ordering::Acquire);
        if flags != 0 {
            let mut update = axfs_ng_vfs::MetadataUpdate::default();
            if flags & 1 != 0 {
                update.atime = Some(axhal::time::realtime_time());
            }
            if flags & 2 != 0 {
                update.mtime = Some(axhal::time::realtime_time());
            }
            if let Err(err) = self.inner.location().update_metadata(update) {
                warn!("Failed to update file times on drop: {err:?}");
            }
        }
    }
}
