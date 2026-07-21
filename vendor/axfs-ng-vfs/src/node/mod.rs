mod dir;
mod file;

use alloc::{
    borrow::ToOwned,
    collections::BTreeMap,
    string::String,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use bitflags::bitflags;
use core::{
    any::{Any, TypeId},
    fmt, iter,
    ops::Deref,
    task::Context,
    time::Duration,
};
use smallvec::SmallVec;

use axpoll::{IoEvents, Pollable};
pub use dir::*;
pub use file::*;
use inherit_methods_macro::inherit_methods;
use log::warn;
use spin::RwLock;

use crate::{
    FilesystemOps, Metadata, MetadataUpdate, Mutex, MutexGuard, NodeType, VfsError, VfsResult,
    path::PathBuf,
};

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct NodeFlags: u32 {
        /// Indicates that this file behaves like a stream.
        ///
        /// Presence of this flag could inform the higher layers to omit
        /// maintaining a position for this file. `read_at` and `write_at` would
        /// be called with zero offset instead.
        const STREAM = 0x0001;

        /// Indicates that this file should not be cached.
        ///
        /// For instance, files in `/proc` or `/sys` may contain dynamic data
        /// that should not be cached.
        const NON_CACHEABLE = 0x0002;

        /// Indicates that this file should always be cached.
        ///
        /// For instance, files in tmpfs relies on page caching and do not have
        /// a backing device.
        const ALWAYS_CACHE = 0x0004;

        /// Indicates that operations on this file are always blocking.
        ///
        /// This could prevent higher layers from attempting to add unnecessary
        /// non-blocking handling.
        const BLOCKING = 0x0008;
    }
}

/// Filesystem node operationss
#[allow(clippy::len_without_is_empty)]
pub trait NodeOps: Send + Sync + 'static {
    /// Gets the inode number of the node.
    fn inode(&self) -> u64;

    /// Gets the metadata of the node.
    fn metadata(&self) -> VfsResult<Metadata>;

    /// Updates the metadata of the node.
    fn update_metadata(&self, update: MetadataUpdate) -> VfsResult<()>;

    /// Gets the filesystem
    fn filesystem(&self) -> &dyn FilesystemOps;

    /// Gets the size of the node.
    fn len(&self) -> VfsResult<u64> {
        self.metadata().map(|m| m.size)
    }

    /// Synchronizes the file to disk.
    fn sync(&self, data_only: bool) -> VfsResult<()>;

    /// Casts the node to a `&dyn core::any::Any`.
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    /// Returns the flags of the node.
    fn flags(&self) -> NodeFlags {
        NodeFlags::empty()
    }

    /// Sets an extended attribute on the node.
    fn set_xattr(&self, _name: &[u8], _value: &[u8]) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    /// Gets an extended attribute from the node.
    fn get_xattr(&self, _name: &[u8], _buf: &mut [u8]) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }

    /// Lists extended attribute names for the node.
    fn list_xattr(&self) -> VfsResult<Vec<u8>> {
        Err(VfsError::OperationNotSupported)
    }

    /// Removes an extended attribute from the node.
    fn remove_xattr(&self, _name: &[u8]) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }
}

enum Node {
    File(FileNode),
    Dir(DirNode),
}

impl Node {
    pub fn clone_inner(&self) -> Arc<dyn NodeOps> {
        match self {
            Node::File(file) => file.inner().clone(),
            Node::Dir(dir) => dir.inner().clone(),
        }
    }
}

impl Deref for Node {
    type Target = dyn NodeOps;

    fn deref(&self) -> &Self::Target {
        match &self {
            Node::File(file) => file.deref(),
            Node::Dir(dir) => dir.deref(),
        }
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::File(file) => write!(f, "FileNode({})", file.inode()),
            Node::Dir(dir) => write!(f, "DirNode({})", dir.inode()),
        }
    }
}

pub type ReferenceKey = (usize, String);

#[derive(Debug)]
pub struct Reference {
    parent: Option<DirEntry>,
    name: String,
}

impl Reference {
    pub fn new(parent: Option<DirEntry>, name: String) -> Self {
        Self { parent, name }
    }

    pub fn root() -> Self {
        Self::new(None, String::new())
    }

    pub fn key(&self) -> ReferenceKey {
        let address = self
            .parent
            .as_ref()
            .map_or(0, |it| Arc::as_ptr(&it.0) as usize);
        (address, self.name.clone())
    }
}

#[derive(Default)]
pub struct TypeMap(SmallVec<[(TypeId, Arc<dyn Any + Send + Sync>); 2]>);
impl TypeMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) {
        self.remove::<T>();
        self.0.push((TypeId::of::<T>(), Arc::new(value)));
    }

    pub fn remove<T: Any + Send + Sync>(&mut self) {
        let id = TypeId::of::<T>();
        self.0.retain(|(ty, _)| ty != &id);
    }

    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.0
            .iter()
            .find_map(|(id, value)| {
                if id == &TypeId::of::<T>() {
                    Some(value.clone())
                } else {
                    None
                }
            })
            .and_then(|value| value.downcast().ok())
    }

    pub fn get_or_insert_with<T: Any + Send + Sync>(&mut self, f: impl FnOnce() -> T) -> Arc<T> {
        if let Some(value) = self.get::<T>() {
            value
        } else {
            let value = f();
            self.insert(value);
            self.get::<T>().unwrap()
        }
    }
}

struct InodeMetadataCache {
    node: Arc<dyn NodeOps>,
    file: Option<Arc<dyn FileNodeOps>>,
    metadata: Metadata,
    dirty_size: bool,
    dirty_update: MetadataUpdate,
}

type InodeMetadataCacheKey = (usize, u64);
type SharedInodeMetadataCache = Arc<RwLock<InodeMetadataCache>>;

static INODE_METADATA_CACHES: spin::Lazy<
    RwLock<BTreeMap<InodeMetadataCacheKey, SharedInodeMetadataCache>>,
> = spin::Lazy::new(|| RwLock::new(BTreeMap::new()));

fn inode_metadata_cache_key(filesystem: &dyn FilesystemOps, inode: u64) -> InodeMetadataCacheKey {
    let fs_ptr: *const dyn FilesystemOps = filesystem;
    (fs_ptr as *const () as usize, inode)
}

pub(super) fn invalidate_inode_metadata_cache(filesystem: &dyn FilesystemOps, inode: u64) {
    let key = inode_metadata_cache_key(filesystem, inode);
    let shared = INODE_METADATA_CACHES.write().remove(&key);
    if let Some(shared) = shared
        && let Err(err) = flush_metadata_cache_shared(&shared, false)
    {
        INODE_METADATA_CACHES.write().entry(key).or_insert(shared);
        warn!("Failed to flush inode metadata cache on invalidation: {err:?}");
    }
}

fn discard_inode_metadata_cache(filesystem: &dyn FilesystemOps, inode: u64) {
    let key = inode_metadata_cache_key(filesystem, inode);
    INODE_METADATA_CACHES.write().remove(&key);
}

fn metadata_update_is_empty(update: &MetadataUpdate) -> bool {
    update.mode.is_none()
        && update.owner.is_none()
        && update.atime.is_none()
        && update.mtime.is_none()
        && update.ctime.is_none()
}

fn should_update_atime(metadata: &Metadata, atime: Duration, strict: bool) -> bool {
    if atime <= metadata.atime {
        return false;
    }
    let stale_by_time = atime
        .checked_sub(metadata.atime)
        .is_some_and(|elapsed| elapsed >= Duration::from_secs(24 * 60 * 60));
    strict || metadata.atime <= metadata.mtime || metadata.atime <= metadata.ctime || stale_by_time
}

fn merge_metadata_update(target: &mut MetadataUpdate, update: &MetadataUpdate) {
    if update.mode.is_some() {
        target.mode = update.mode;
    }
    if update.owner.is_some() {
        target.owner = update.owner;
    }
    if update.atime.is_some() {
        target.atime = update.atime;
    }
    if update.mtime.is_some() {
        target.mtime = update.mtime;
    }
    if update.ctime.is_some() {
        target.ctime = update.ctime;
    }
}

fn apply_metadata_update(metadata: &mut Metadata, update: &MetadataUpdate) {
    if let Some(mode) = update.mode {
        metadata.mode = mode;
    }
    if let Some((uid, gid)) = update.owner {
        metadata.uid = uid;
        metadata.gid = gid;
    }
    if let Some(atime) = update.atime {
        metadata.atime = atime;
    }
    if let Some(mtime) = update.mtime {
        metadata.mtime = mtime;
    }
    if let Some(ctime) = update.ctime {
        metadata.ctime = ctime;
    }
}

fn restore_metadata_dirty(
    shared: &SharedInodeMetadataCache,
    size: Option<u64>,
    update: &MetadataUpdate,
) {
    let mut cache = shared.write();
    if size.is_some() {
        cache.dirty_size = true;
    }
    merge_metadata_update(&mut cache.dirty_update, update);
}

fn flush_metadata_cache_shared(
    shared: &SharedInodeMetadataCache,
    data_only: bool,
) -> VfsResult<()> {
    let (size, update, file, node) = {
        let mut cache = shared.write();
        let size = cache.dirty_size.then_some(cache.metadata.size);
        let update = if data_only {
            MetadataUpdate::default()
        } else {
            cache.dirty_update.clone()
        };
        if size.is_none() && metadata_update_is_empty(&update) {
            return Ok(());
        }
        if size.is_some() {
            cache.dirty_size = false;
        }
        if !data_only {
            cache.dirty_update = MetadataUpdate::default();
        }
        (size, update, cache.file.clone(), cache.node.clone())
    };

    if let Some(size) = size {
        let Some(file) = file else {
            restore_metadata_dirty(shared, Some(size), &update);
            return Err(VfsError::InvalidInput);
        };
        if let Err(err) = file.set_len(size) {
            restore_metadata_dirty(shared, Some(size), &update);
            return Err(err);
        }
    }
    if !metadata_update_is_empty(&update)
        && let Err(err) = node.update_metadata(update.clone())
    {
        restore_metadata_dirty(shared, None, &update);
        return Err(err);
    }
    Ok(())
}

pub fn flush_inode_metadata_caches(data_only: bool) -> VfsResult<()> {
    let caches = INODE_METADATA_CACHES
        .read()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for cache in caches {
        flush_metadata_cache_shared(&cache, data_only)?;
    }
    Ok(())
}

struct Inner {
    node: Node,
    node_type: NodeType,
    reference: Reference,
    metadata_cache: RwLock<Weak<RwLock<InodeMetadataCache>>>,
    user_data: Mutex<TypeMap>,
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("node", &self.node)
            .field("node_type", &self.node_type)
            .field("reference", &self.reference)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct DirEntry(Arc<Inner>);

#[derive(Debug, Clone)]
pub struct WeakDirEntry(Weak<Inner>);

impl WeakDirEntry {
    pub fn upgrade(&self) -> Option<DirEntry> {
        self.0.upgrade().map(DirEntry)
    }
}

impl From<Node> for Arc<dyn NodeOps> {
    fn from(node: Node) -> Self {
        match node {
            Node::File(file) => file.into(),
            Node::Dir(dir) => dir.into(),
        }
    }
}

#[inherit_methods(from = "self.0.node")]
impl DirEntry {
    pub fn inode(&self) -> u64;

    pub fn filesystem(&self) -> &dyn FilesystemOps;

    pub fn flags(&self) -> NodeFlags;

    pub fn set_xattr(&self, name: &[u8], value: &[u8]) -> VfsResult<()>;

    pub fn get_xattr(&self, name: &[u8], buf: &mut [u8]) -> VfsResult<usize>;

    pub fn list_xattr(&self) -> VfsResult<Vec<u8>>;

    pub fn remove_xattr(&self, name: &[u8]) -> VfsResult<()>;
}

impl DirEntry {
    pub fn new_file(node: FileNode, node_type: NodeType, reference: Reference) -> Self {
        Self(Arc::new(Inner {
            node: Node::File(node),
            node_type,
            reference,
            metadata_cache: RwLock::new(Weak::new()),
            user_data: Mutex::default(),
        }))
    }

    pub fn new_dir(node_fn: impl FnOnce(WeakDirEntry) -> DirNode, reference: Reference) -> Self {
        Self(Arc::new_cyclic(|this| Inner {
            node: Node::Dir(node_fn(WeakDirEntry(this.clone()))),
            node_type: NodeType::Directory,
            reference,
            metadata_cache: RwLock::new(Weak::new()),
            user_data: Mutex::default(),
        }))
    }

    fn cacheable_metadata(&self) -> bool {
        self.node_type() != NodeType::Directory && !self.flags().contains(NodeFlags::NON_CACHEABLE)
    }

    fn metadata_cache_key(&self) -> InodeMetadataCacheKey {
        inode_metadata_cache_key(self.filesystem(), self.inode())
    }

    fn raw_metadata(&self) -> VfsResult<Metadata> {
        self.0.node.metadata().map(|mut metadata| {
            metadata.node_type = self.0.node_type;
            metadata
        })
    }

    fn metadata_cache(&self) -> Option<SharedInodeMetadataCache> {
        if let Some(cache) = self.0.metadata_cache.read().upgrade() {
            return Some(cache);
        }

        let cache = INODE_METADATA_CACHES
            .read()
            .get(&self.metadata_cache_key())
            .cloned();
        if let Some(cache) = &cache {
            *self.0.metadata_cache.write() = Arc::downgrade(cache);
        }
        cache
    }

    fn ensure_metadata_cache(&self) -> VfsResult<SharedInodeMetadataCache> {
        if let Some(cache) = self.metadata_cache() {
            return Ok(cache);
        }

        let key = self.metadata_cache_key();
        let metadata = self.raw_metadata()?;
        let mut caches = INODE_METADATA_CACHES.write();
        let cache = caches
            .entry(key)
            .or_insert_with(|| {
                Arc::new(RwLock::new(InodeMetadataCache {
                    node: self.0.node.clone_inner(),
                    file: self.as_file().ok().map(|file| file.inner().clone()),
                    metadata,
                    dirty_size: false,
                    dirty_update: MetadataUpdate::default(),
                }))
            })
            .clone();
        *self.0.metadata_cache.write() = Arc::downgrade(&cache);
        Ok(cache)
    }

    pub fn metadata(&self) -> VfsResult<Metadata> {
        if !self.cacheable_metadata() {
            return self.raw_metadata();
        }
        if let Some(cache) = self.metadata_cache() {
            return Ok(cache.read().metadata.clone());
        }
        Ok(self.ensure_metadata_cache()?.read().metadata.clone())
    }

    pub fn cached_metadata(&self) -> Option<Metadata> {
        self.metadata_cache()
            .map(|cache| cache.read().metadata.clone())
    }

    pub fn invalidate_metadata_cache(&self) {
        *self.0.metadata_cache.write() = Weak::new();
        invalidate_inode_metadata_cache(self.filesystem(), self.inode())
    }

    pub fn discard_metadata_cache(&self) {
        *self.0.metadata_cache.write() = Weak::new();
        discard_inode_metadata_cache(self.filesystem(), self.inode())
    }

    pub fn update_metadata(&self, update: MetadataUpdate) -> VfsResult<()> {
        if !self.cacheable_metadata() {
            return self.0.node.update_metadata(update);
        }

        let shared = self.ensure_metadata_cache()?;
        let mut cache = shared.write();
        apply_metadata_update(&mut cache.metadata, &update);
        merge_metadata_update(&mut cache.dirty_update, &update);
        Ok(())
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> VfsResult<u64> {
        if !self.cacheable_metadata() {
            return self.0.node.len();
        }
        self.metadata().map(|metadata| metadata.size)
    }

    pub fn grow_cached_len(&self, len: u64, dirty: bool) -> VfsResult<u64> {
        if !self.cacheable_metadata() {
            return self.raw_metadata().map(|metadata| metadata.size);
        }
        let shared = self.ensure_metadata_cache()?;
        let mut cache = shared.write();
        let old_len = cache.metadata.size;
        if len > old_len {
            cache.metadata.size = len;
            cache.dirty_size |= dirty;
        }
        Ok(old_len)
    }

    pub fn set_cached_len(&self, len: u64, dirty: bool) -> VfsResult<()> {
        if !self.cacheable_metadata() {
            return Ok(());
        }
        let shared = self.ensure_metadata_cache()?;
        let mut cache = shared.write();
        cache.metadata.size = len;
        cache.dirty_size = dirty;
        Ok(())
    }

    pub fn set_cached_len_if_present(&self, len: u64) {
        if let Some(shared) = self.metadata_cache() {
            let mut cache = shared.write();
            cache.metadata.size = len;
            cache.dirty_size = false;
        }
    }

    pub fn grow_cached_len_if_present(&self, len: u64) {
        if let Some(shared) = self.metadata_cache() {
            let mut cache = shared.write();
            if len > cache.metadata.size {
                cache.metadata.size = len;
            }
        }
    }

    pub fn record_access(&self, atime: Duration, strict: bool) -> VfsResult<bool> {
        if !self.cacheable_metadata() {
            return Ok(false);
        }
        let shared = self.ensure_metadata_cache()?;
        {
            let cache = shared.read();
            if !should_update_atime(&cache.metadata, atime, strict) {
                return Ok(true);
            }
        }

        // Recheck after upgrading: another reader may already have refreshed
        // atime while this task was waiting for the writer.
        let mut cache = shared.write();
        if should_update_atime(&cache.metadata, atime, strict) {
            cache.metadata.atime = atime;
            cache.dirty_update.atime = Some(atime);
        }
        Ok(true)
    }

    pub fn record_modification(&self, mtime: Duration) -> VfsResult<bool> {
        if !self.cacheable_metadata() {
            return Ok(false);
        }
        let shared = self.ensure_metadata_cache()?;
        let mut cache = shared.write();
        cache.metadata.mtime = mtime;
        cache.metadata.ctime = mtime;
        cache.dirty_update.mtime = Some(mtime);
        cache.dirty_update.ctime = Some(mtime);
        Ok(true)
    }

    pub fn flush_metadata_cache(&self, data_only: bool) -> VfsResult<()> {
        if !self.cacheable_metadata() {
            return Ok(());
        }
        let Some(shared) = self.metadata_cache() else {
            return Ok(());
        };
        flush_metadata_cache_shared(&shared, data_only)
    }

    pub fn sync(&self, data_only: bool) -> VfsResult<()> {
        self.flush_metadata_cache(data_only)?;
        self.0.node.sync(data_only)
    }

    pub fn downcast<T: NodeOps>(&self) -> VfsResult<Arc<T>> {
        self.0
            .node
            .clone_inner()
            .into_any()
            .downcast()
            .map_err(|_| VfsError::InvalidInput)
    }

    pub fn downgrade(&self) -> WeakDirEntry {
        WeakDirEntry(Arc::downgrade(&self.0))
    }

    pub fn key(&self) -> ReferenceKey {
        self.0.reference.key()
    }

    pub fn node_type(&self) -> NodeType {
        self.0.node_type
    }

    pub fn parent(&self) -> Option<Self> {
        self.0.reference.parent.clone()
    }

    pub fn name(&self) -> &str {
        &self.0.reference.name
    }

    /// Checks if the entry is a root of a mount point.
    pub fn is_root_of_mount(&self) -> bool {
        self.0.reference.parent.is_none()
    }

    pub fn is_ancestor_of(&self, other: &Self) -> VfsResult<bool> {
        let mut current = other.clone();
        loop {
            if current.ptr_eq(self) {
                return Ok(true);
            }
            if let Some(parent) = current.parent() {
                current = parent;
            } else {
                break;
            }
        }
        Ok(false)
    }

    pub(crate) fn collect_absolute_path(&self, components: &mut Vec<String>) {
        let mut current = self.clone();
        loop {
            components.push(current.name().to_owned());
            if let Some(parent) = current.parent() {
                current = parent;
            } else {
                break;
            }
        }
    }

    pub fn absolute_path(&self) -> VfsResult<PathBuf> {
        let mut components = vec![];
        self.collect_absolute_path(&mut components);
        Ok(iter::once("/")
            .chain(components.iter().map(String::as_str).rev())
            .collect())
    }

    pub fn is_file(&self) -> bool {
        matches!(self.0.node, Node::File(_))
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.0.node, Node::Dir(_))
    }

    pub fn as_file(&self) -> VfsResult<&FileNode> {
        match &self.0.node {
            Node::File(file) => Ok(file),
            _ => Err(VfsError::IsADirectory),
        }
    }

    pub fn as_dir(&self) -> VfsResult<&DirNode> {
        match &self.0.node {
            Node::Dir(dir) => Ok(dir),
            _ => Err(VfsError::NotADirectory),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn as_ptr(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    pub fn read_link(&self) -> VfsResult<String> {
        if self.node_type() != NodeType::Symlink {
            return Err(VfsError::InvalidData);
        }
        let file = self.as_file()?;
        let mut buf = vec![0; self.len()? as usize];
        file.read_at(&mut buf, 0)?;
        String::from_utf8(buf).map_err(|_| VfsError::InvalidData)
    }

    pub fn ioctl(&self, cmd: u32, arg: usize) -> VfsResult<usize> {
        match &self.0.node {
            Node::File(file) => file.ioctl(cmd, arg),
            Node::Dir(_) => Err(VfsError::NotATty),
        }
    }

    pub fn user_data(&self) -> MutexGuard<'_, TypeMap> {
        self.0.user_data.lock()
    }
}

impl Pollable for DirEntry {
    fn poll(&self) -> IoEvents {
        match &self.0.node {
            Node::File(file) => file.poll(),
            Node::Dir(_dir) => IoEvents::IN | IoEvents::OUT,
        }
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        match &self.0.node {
            Node::File(file) => file.register(context, events),
            Node::Dir(_) => {}
        }
    }
}
