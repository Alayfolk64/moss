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
        self.0.push((TypeId::of::<T>(), Arc::new(value)));
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
    metadata: Metadata,
    dirty_size: bool,
    dirty_atime: bool,
}

type InodeMetadataCacheKey = (usize, u64);
type SharedInodeMetadataCache = Arc<Mutex<InodeMetadataCache>>;

static INODE_METADATA_CACHES: spin::Lazy<Mutex<BTreeMap<InodeMetadataCacheKey, SharedInodeMetadataCache>>> =
    spin::Lazy::new(|| Mutex::new(BTreeMap::new()));

fn inode_metadata_cache_key(filesystem: &dyn FilesystemOps, inode: u64) -> InodeMetadataCacheKey {
    let fs_ptr: *const dyn FilesystemOps = filesystem;
    (fs_ptr as *const () as usize, inode)
}

pub(super) fn invalidate_inode_metadata_cache(filesystem: &dyn FilesystemOps, inode: u64) {
    INODE_METADATA_CACHES
        .lock()
        .remove(&inode_metadata_cache_key(filesystem, inode));
}

struct Inner {
    node: Node,
    node_type: NodeType,
    reference: Reference,
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
            user_data: Mutex::default(),
        }))
    }

    pub fn new_dir(node_fn: impl FnOnce(WeakDirEntry) -> DirNode, reference: Reference) -> Self {
        Self(Arc::new_cyclic(|this| Inner {
            node: Node::Dir(node_fn(WeakDirEntry(this.clone()))),
            node_type: NodeType::Directory,
            reference,
            user_data: Mutex::default(),
        }))
    }

    fn cacheable_metadata(&self) -> bool {
        !self.flags().contains(NodeFlags::NON_CACHEABLE)
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
        INODE_METADATA_CACHES
            .lock()
            .get(&self.metadata_cache_key())
            .cloned()
    }

    fn ensure_metadata_cache(&self) -> VfsResult<SharedInodeMetadataCache> {
        if let Some(cache) = self.metadata_cache() {
            return Ok(cache);
        }

        let key = self.metadata_cache_key();
        let metadata = self.raw_metadata()?;
        let mut caches = INODE_METADATA_CACHES.lock();
        Ok(caches
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Mutex::new(InodeMetadataCache {
                    metadata,
                    dirty_size: false,
                    dirty_atime: false,
                }))
            })
            .clone())
    }

    fn update_or_insert_metadata_cache(
        &self,
        metadata: Metadata,
        f: impl FnOnce(&mut InodeMetadataCache, Metadata),
    ) {
        let key = self.metadata_cache_key();
        let mut caches = INODE_METADATA_CACHES.lock();
        let cache = caches
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Mutex::new(InodeMetadataCache {
                    metadata: metadata.clone(),
                    dirty_size: false,
                    dirty_atime: false,
                }))
            })
            .clone();
        drop(caches);
        let mut cache = cache.lock();
        f(&mut *cache, metadata);
    }

    pub fn metadata(&self) -> VfsResult<Metadata> {
        if !self.cacheable_metadata() {
            return self.raw_metadata();
        }
        if let Some(cache) = self.metadata_cache() {
            return Ok(cache.lock().metadata.clone());
        }
        Ok(self.ensure_metadata_cache()?.lock().metadata.clone())
    }

    pub fn cached_metadata(&self) -> Option<Metadata> {
        self.metadata_cache()
            .map(|cache| cache.lock().metadata.clone())
    }

    pub fn invalidate_metadata_cache(&self) {
        invalidate_inode_metadata_cache(self.filesystem(), self.inode())
    }

    pub fn update_metadata(&self, update: MetadataUpdate) -> VfsResult<()> {
        self.0.node.update_metadata(update.clone())?;
        if !self.cacheable_metadata() {
            return Ok(());
        }

        let metadata = self.raw_metadata()?;
        self.update_or_insert_metadata_cache(metadata, |cache, mut metadata| {
            let cached_size = cache.metadata.size;
            let dirty_size = cache.dirty_size;
            let dirty_atime = cache.dirty_atime && update.atime.is_none();
            metadata.size = cached_size;
            if dirty_atime {
                metadata.atime = cache.metadata.atime;
            }
            *cache = InodeMetadataCache {
                metadata,
                dirty_size,
                dirty_atime,
            };
        });
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
        let mut cache = shared.lock();
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
        let mut cache = shared.lock();
        cache.metadata.size = len;
        cache.dirty_size = dirty;
        Ok(())
    }

    pub fn set_cached_len_if_present(&self, len: u64) {
        if let Some(shared) = self.metadata_cache() {
            let mut cache = shared.lock();
            cache.metadata.size = len;
            cache.dirty_size = false;
        }
    }

    pub fn grow_cached_len_if_present(&self, len: u64) {
        if let Some(shared) = self.metadata_cache() {
            let mut cache = shared.lock();
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
        let mut cache = shared.lock();
        let stale_by_time = atime
            .checked_sub(cache.metadata.atime)
            .map_or(false, |elapsed| elapsed >= Duration::from_secs(24 * 60 * 60));
        let should_update = strict
            || cache.metadata.atime <= cache.metadata.mtime
            || cache.metadata.atime <= cache.metadata.ctime
            || stale_by_time;
        if should_update && atime > cache.metadata.atime {
            cache.metadata.atime = atime;
            cache.dirty_atime = true;
        }
        Ok(true)
    }

    pub fn flush_metadata_cache(&self, data_only: bool) -> VfsResult<()> {
        if !self.cacheable_metadata() {
            return Ok(());
        }
        let Some(shared) = self.metadata_cache() else {
            return Ok(());
        };
        let (size, atime) = {
            let mut cache = shared.lock();
            let size = cache.dirty_size.then_some(cache.metadata.size);
            let atime = (!data_only && cache.dirty_atime).then_some(cache.metadata.atime);
            if size.is_none() && atime.is_none() {
                return Ok(());
            }
            cache.dirty_size &= size.is_none();
            cache.dirty_atime &= atime.is_none();
            (size, atime)
        };

        if let Some(size) = size {
            if let Err(err) = self.as_file()?.set_len(size) {
                let mut cache = shared.lock();
                cache.dirty_size = true;
                if atime.is_some() {
                    cache.dirty_atime = true;
                }
                return Err(err);
            }
        }
        if let Some(atime) = atime
            && let Err(err) = self.0.node.update_metadata(MetadataUpdate {
                atime: Some(atime),
                ..Default::default()
            })
        {
            shared.lock().dirty_atime = true;
            return Err(err);
        }
        Ok(())
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
