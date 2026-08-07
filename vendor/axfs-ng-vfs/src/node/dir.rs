use alloc::{borrow::ToOwned, string::String, sync::Arc};
use core::{
    mem,
    ops::Deref,
    sync::atomic::{AtomicU64, Ordering},
};

use axsync::{Mutex as SleepMutex, MutexGuard as SleepMutexGuard};
use hashbrown::{HashMap, HashSet};
use spin::{RwLock, RwLockWriteGuard};

use super::{DirEntry, invalidate_inode_metadata_cache};
use crate::{
    MetadataUpdate, Mountpoint, Mutex, NodeOps, NodePermission, NodeType, VfsError, VfsResult,
    path::{DOT, DOTDOT, MAX_NAME_LEN, verify_entry_name},
};

const LOOKUP_LOCK_SHARDS: usize = 16;

/// A trait for a sink that can receive directory entries.
pub trait DirEntrySink {
    /// Accept a directory entry, returns `false` if the sink is full.
    ///
    /// `offset` is the offset of the next entry to be read.
    ///
    /// It's not recommended to operate on the node inside the `accept`
    /// function, since some filesystem may impose a lock while iterating the
    /// directory, and operating on the node may cause deadlock.
    fn accept(&mut self, name: &str, ino: u64, node_type: NodeType, offset: u64) -> bool;
}

impl<F: FnMut(&str, u64, NodeType, u64) -> bool> DirEntrySink for F {
    fn accept(&mut self, name: &str, ino: u64, node_type: NodeType, offset: u64) -> bool {
        self(name, ino, node_type, offset)
    }
}

#[derive(Default)]
struct DirCache {
    children: HashMap<String, DirEntry>,
    /// Names observed absent in this directory. A successful namespace
    /// mutation removes or invalidates the corresponding entry before
    /// publishing its new generation.
    negative: HashSet<String>,
}

pub trait DirNodeOps: NodeOps {
    /// Reads directory entries.
    ///
    /// Returns the number of entries read.
    ///
    /// Implementations should ensure that `.` and `..` are present in the
    /// result.
    fn read_dir(&self, offset: u64, sink: &mut dyn DirEntrySink) -> VfsResult<usize>;

    /// Lookups a directory entry by name.
    fn lookup(&self, name: &str) -> VfsResult<DirEntry>;

    /// Returns whether directory entries can be cached.
    ///
    /// Some filesystems (like '/proc') may not support caching directory
    /// entries, as they may change frequently or not be backed by persistent
    /// storage.
    ///
    /// If this returns `false`, the directory will not be cached in dentry and
    /// each call to [`DirNode::lookup`] will end up calling [`lookup`].
    /// Implementations should take care to handle cases where [`lookup`] is
    /// called multiple times for the same name.
    fn is_cacheable(&self) -> bool {
        true
    }

    /// Creates a directory entry.
    fn create(
        &self,
        name: &str,
        node_type: NodeType,
        permission: NodePermission,
    ) -> VfsResult<DirEntry>;

    /// Creates an entry after the VFS has looked up `name` under the same
    /// directory mutation lock and observed `NotFound`.
    ///
    /// Filesystems may override this to avoid repeating an expensive
    /// directory scan. The default retains the regular checked create path.
    fn create_after_lookup(
        &self,
        name: &str,
        node_type: NodeType,
        permission: NodePermission,
    ) -> VfsResult<DirEntry> {
        self.create(name, node_type, permission)
    }

    /// Creates a link to a node.
    fn link(&self, name: &str, node: &DirEntry) -> VfsResult<DirEntry>;

    /// Unlinks a directory entry by name.
    ///
    /// If the entry is a non-empty directory, it should return `ENOTEMPTY`
    /// error.
    fn unlink(&self, name: &str) -> VfsResult<()>;

    /// Renames a directory entry, replacing the original entry (dst) if it
    /// already exists.
    ///
    /// If src and dst link to the same file, this should do nothing and return
    /// `Ok(())`.
    ///
    /// The caller should ensure:
    /// - If `src` is a directory, `dst` must not exist or be an empty
    ///   directory.
    /// - If `src` is not a directory, `dst` must not exist or not be a
    ///   directory.
    fn rename(&self, src_name: &str, dst_dir: &DirNode, dst_name: &str) -> VfsResult<()>;
}

/// Options for opening (or creating) a directory entry.
///
/// See [`DirNode::open_file`] for more details.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub create: bool,
    pub create_new: bool,
    pub node_type: NodeType,
    pub permission: NodePermission,
    pub user: Option<(u32, u32)>, // (uid, gid)
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            create: false,
            create_new: false,
            node_type: NodeType::RegularFile,
            permission: NodePermission::default(),
            user: None,
        }
    }
}

pub struct DirNode {
    ops: Arc<dyn DirNodeOps>,
    cache: RwLock<DirCache>,
    cache_generation: AtomicU64,
    lookup_locks: [SleepMutex<()>; LOOKUP_LOCK_SHARDS],
    mutation_lock: SleepMutex<()>,
    pub(crate) mountpoint: Mutex<Option<Arc<Mountpoint>>>,
}

impl Deref for DirNode {
    type Target = dyn NodeOps;

    fn deref(&self) -> &Self::Target {
        &*self.ops
    }
}

impl From<DirNode> for Arc<dyn NodeOps> {
    fn from(node: DirNode) -> Self {
        node.ops.clone()
    }
}

impl DirNode {
    pub fn new(ops: Arc<dyn DirNodeOps>) -> Self {
        Self {
            ops,
            cache: RwLock::default(),
            cache_generation: AtomicU64::new(0),
            lookup_locks: [const { SleepMutex::new(()) }; LOOKUP_LOCK_SHARDS],
            mutation_lock: SleepMutex::new(()),
            mountpoint: Mutex::default(),
        }
    }

    pub fn inner(&self) -> &Arc<dyn DirNodeOps> {
        &self.ops
    }

    pub fn downcast<T: DirNodeOps>(&self) -> VfsResult<Arc<T>> {
        self.ops
            .clone()
            .into_any()
            .downcast()
            .map_err(|_| VfsError::InvalidInput)
    }

    fn forget_entry(cache: &mut DirCache, name: &str) {
        if let Some(entry) = cache.children.remove(name)
            && let Ok(dir) = entry.as_dir()
        {
            dir.forget();
        }
    }

    fn cache_positive(cache: &mut DirCache, name: &str, entry: DirEntry) {
        cache.negative.remove(name);
        cache.children.insert(name.to_owned(), entry);
    }

    fn cache_negative(cache: &mut DirCache, name: &str) {
        Self::forget_entry(cache, name);
        cache.negative.insert(name.to_owned());
    }

    /// Publishes a filesystem lookup result without retaining the cache lock
    /// across the potentially blocking lookup itself.
    fn publish_lookup_result(
        &self,
        name: &str,
        result: &VfsResult<DirEntry>,
    ) {
        if !self.ops.is_cacheable() {
            return;
        }
        let mut cache = self.cache.write();
        match result {
            Ok(entry) => {
                Self::cache_positive(&mut cache, name, entry.clone());
            }
            Err(err) => {
                if err.canonicalize() == VfsError::NotFound {
                    Self::cache_negative(&mut cache, name);
                }
            }
        }
    }

    /// Performs a real filesystem lookup while the caller holds this
    /// directory's mutation lock.
    fn lookup_fresh_for_mutation(&self, name: &str) -> VfsResult<DirEntry> {
        let result = self.ops.lookup(name);
        self.publish_lookup_result(name, &result);
        result
    }

    /// Looks in the positive dentry cache before falling back to a real lookup.
    /// The caller holds `mutation_lock`, so a cached result cannot be
    /// invalidated by another local mutation during this operation.
    fn lookup_for_mutation(&self, name: &str) -> VfsResult<DirEntry> {
        if self.ops.is_cacheable() {
            let cache = self.cache.read();
            if let Some(entry) = cache.children.get(name) {
                return Ok(entry.clone());
            }
            if cache.negative.contains(name) {
                return Err(VfsError::NotFound);
            }
        }
        self.lookup_fresh_for_mutation(name)
    }

    fn lookup_lock(&self, name: &str) -> &SleepMutex<()> {
        let hash = name.bytes().fold(0usize, |hash, byte| {
            hash.wrapping_mul(33).wrapping_add(byte as usize)
        });
        &self.lookup_locks[hash % LOOKUP_LOCK_SHARDS]
    }

    /// Looks up a directory entry by name.
    pub fn lookup(&self, name: &str) -> VfsResult<DirEntry> {
        if name.len() > MAX_NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        if !self.ops.is_cacheable() {
            return self.ops.lookup(name);
        }

        {
            let cache = self.cache.read();
            if let Some(entry) = cache.children.get(name).cloned() {
                return Ok(entry);
            }
            if cache.negative.contains(name) {
                return Err(VfsError::NotFound);
            }
        }

        // Coalesce concurrent misses for the same name. This is a sleeping,
        // per-directory sharded gate rather than a cache lock: different names
        // can still progress, and followers recheck the dentry cache instead
        // of repeating the same filesystem lookup.
        let _lookup = self.lookup_lock(name).lock();
        let generation = self.cache_generation.load(Ordering::Acquire);
        {
            let cache = self.cache.read();
            if let Some(entry) = cache.children.get(name).cloned() {
                return Ok(entry);
            }
            if cache.negative.contains(name) {
                return Err(VfsError::NotFound);
            }
        }

        // Filesystem lookup may block on metadata I/O. Do not hold the
        // directory cache write lock across it: a cold lookup for one name
        // must not block cache hits for every other name in the directory.
        let result = self.ops.lookup(name);
        if self.cache_generation.load(Ordering::Acquire) == generation {
            let mut cache = self.cache.write();
            if self.cache_generation.load(Ordering::Acquire) == generation {
                match &result {
                    Ok(entry) => {
                        Self::cache_positive(&mut cache, name, entry.clone());
                    }
                    Err(err) if err.canonicalize() == VfsError::NotFound => {
                        Self::cache_negative(&mut cache, name);
                    }
                    Err(_) => {}
                }
            }
        }
        result
    }

    /// Looks up a directory entry by name without trusting a cached dentry.
    pub fn lookup_fresh(&self, name: &str) -> VfsResult<DirEntry> {
        if name.len() > MAX_NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        let _mutation = self.mutation_lock.lock();
        self.lookup_fresh_for_mutation(name)
    }

    /// Looks up a directory entry by name in cache.
    pub fn lookup_cache(&self, name: &str) -> Option<DirEntry> {
        if self.ops.is_cacheable() {
            self.cache.read().children.get(name).cloned()
        } else {
            None
        }
    }

    /// Inserts a directory entry into the cache.
    pub fn insert_cache(&self, name: String, entry: DirEntry) -> Option<DirEntry> {
        if self.ops.is_cacheable() {
            let mut cache = self.cache.write();
            cache.negative.remove(&name);
            let previous = cache.children.insert(name, entry);
            self.cache_generation.fetch_add(1, Ordering::AcqRel);
            previous
        } else {
            None
        }
    }

    pub fn read_dir(&self, offset: u64, sink: &mut dyn DirEntrySink) -> VfsResult<usize> {
        self.ops.read_dir(offset, sink)
    }

    /// Creates a link to a node.
    pub fn link(&self, name: &str, node: &DirEntry) -> VfsResult<DirEntry> {
        verify_entry_name(name)?;

        let _mutation = self.mutation_lock.lock();
        self.ops.link(name, node).inspect(|entry| {
            invalidate_inode_metadata_cache(self.filesystem(), self.inode());
            invalidate_inode_metadata_cache(node.filesystem(), node.inode());
            let mut cache = self.cache.write();
            if self.ops.is_cacheable() {
                Self::cache_positive(&mut cache, name, entry.clone());
            }
            self.cache_generation.fetch_add(1, Ordering::AcqRel);
        })
    }

    /// Unlinks a directory entry by name.
    pub fn unlink(&self, name: &str, is_dir: bool) -> VfsResult<()> {
        verify_entry_name(name)?;

        let _mutation = self.mutation_lock.lock();
        let entry = self.lookup_fresh_for_mutation(name)?;
        match (entry.is_dir(), is_dir) {
            (true, false) => return Err(VfsError::IsADirectory),
            (false, true) => return Err(VfsError::NotADirectory),
            _ => {}
        }

        match self.ops.unlink(name) {
            Ok(()) => {
                invalidate_inode_metadata_cache(self.filesystem(), self.inode());
                invalidate_inode_metadata_cache(entry.filesystem(), entry.inode());
                let mut cache = self.cache.write();
                Self::forget_entry(&mut cache, name);
                if self.ops.is_cacheable() {
                    cache.negative.insert(name.to_owned());
                }
                self.cache_generation.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            Err(err) => {
                if err.canonicalize() == VfsError::NotFound {
                    let mut cache = self.cache.write();
                    Self::forget_entry(&mut cache, name);
                }
                Err(err)
            }
        }
    }

    /// Returns whether the directory contains children.
    pub fn has_children(&self) -> VfsResult<bool> {
        let mut has_children = false;
        self.read_dir(0, &mut |name: &str, _, _, _| {
            if name != DOT && name != DOTDOT {
                has_children = true;
                false
            } else {
                true
            }
        })?;
        Ok(has_children)
    }

    fn create_for_mutation(
        &self,
        name: &str,
        node_type: NodeType,
        permission: NodePermission,
        known_absent: bool,
    ) -> VfsResult<DirEntry> {
        let entry = if known_absent {
            self.ops
                .create_after_lookup(name, node_type, permission)?
        } else {
            self.ops.create(name, node_type, permission)?
        };
        invalidate_inode_metadata_cache(self.filesystem(), self.inode());
        let mut cache = self.cache.write();
        Self::forget_entry(&mut cache, name);
        if self.ops.is_cacheable() {
            Self::cache_positive(&mut cache, name, entry.clone());
        }
        self.cache_generation.fetch_add(1, Ordering::AcqRel);
        Ok(entry)
    }

    /// Creates a directory entry.
    pub fn create(
        &self,
        name: &str,
        node_type: NodeType,
        permission: NodePermission,
    ) -> VfsResult<DirEntry> {
        verify_entry_name(name)?;
        let _mutation = self.mutation_lock.lock();
        self.create_for_mutation(name, node_type, permission, false)
    }

    fn lock_both_mutations<'a>(
        &'a self,
        other: &'a Self,
    ) -> (
        SleepMutexGuard<'a, ()>,
        Option<SleepMutexGuard<'a, ()>>,
    ) {
        if core::ptr::eq(self, other) {
            (self.mutation_lock.lock(), None)
        } else if (self as *const Self as usize) < (other as *const Self as usize) {
            let src_guard = self.mutation_lock.lock();
            let dst_guard = other.mutation_lock.lock();
            (src_guard, Some(dst_guard))
        } else {
            let dst_guard = other.mutation_lock.lock();
            let src_guard = self.mutation_lock.lock();
            (src_guard, Some(dst_guard))
        }
    }

    fn lock_both_cache<'a>(
        &'a self,
        other: &'a Self,
    ) -> (
        RwLockWriteGuard<'a, DirCache>,
        Option<RwLockWriteGuard<'a, DirCache>>,
    ) {
        if core::ptr::eq(self, other) {
            (self.cache.write(), None)
        } else if (self as *const Self as usize) < (other as *const Self as usize) {
            let src_children = self.cache.write();
            let dst_children = other.cache.write();
            (src_children, Some(dst_children))
        } else {
            let dst_children = other.cache.write();
            let src_children = self.cache.write();
            (src_children, Some(dst_children))
        }
    }

    /// Renames a directory entry.
    pub fn rename(&self, src_name: &str, dst_dir: &Self, dst_name: &str) -> VfsResult<()> {
        verify_entry_name(src_name)?;
        verify_entry_name(dst_name)?;

        if core::ptr::eq(self, dst_dir) && src_name == dst_name {
            return Ok(());
        }

        let (_src_mutation, _dst_mutation) = self.lock_both_mutations(dst_dir);
        let src = self.lookup_fresh_for_mutation(src_name)?;
        let dst = match dst_dir.lookup_fresh_for_mutation(dst_name) {
            Ok(entry) => Some(entry),
            Err(err) if err.canonicalize() == VfsError::NotFound => None,
            Err(err) => return Err(err),
        };
        if let Some(dst) = dst.as_ref() {
            if src.node_type() == NodeType::Directory {
                if let Ok(dir) = dst.as_dir()
                    && dir.has_children()?
                {
                    return Err(VfsError::DirectoryNotEmpty);
                }
            } else if dst.node_type() == NodeType::Directory {
                return Err(VfsError::IsADirectory);
            }
        }

        self.ops.rename(src_name, dst_dir, dst_name).inspect(|_| {
            invalidate_inode_metadata_cache(self.filesystem(), self.inode());
            if !core::ptr::eq(self, dst_dir) {
                invalidate_inode_metadata_cache(dst_dir.filesystem(), dst_dir.inode());
            }
            invalidate_inode_metadata_cache(src.filesystem(), src.inode());
            if let Some(dst) = dst.as_ref() {
                invalidate_inode_metadata_cache(dst.filesystem(), dst.inode());
            }
            let (mut src_children, mut dst_children) = self.lock_both_cache(dst_dir);
            Self::forget_entry(&mut src_children, src_name);
            if self.ops.is_cacheable() {
                src_children.negative.insert(src_name.to_owned());
            }
            let dst_cache: &mut DirCache = match dst_children.as_mut() {
                Some(cache) => &mut **cache,
                None => &mut src_children,
            };
            Self::forget_entry(dst_cache, dst_name);
            dst_cache.negative.remove(dst_name);
            self.cache_generation.fetch_add(1, Ordering::AcqRel);
            if !core::ptr::eq(self, dst_dir) {
                dst_dir.cache_generation.fetch_add(1, Ordering::AcqRel);
            }
        })
    }

    /// Opens (or creates) a file in the directory.
    pub fn open_file(&self, name: &str, options: &OpenOptions) -> VfsResult<DirEntry> {
        verify_entry_name(name)?;

        if !options.create && !options.create_new {
            return self.lookup(name);
        }

        let _mutation = self.mutation_lock.lock();
        if options.create && !options.create_new {
            // Most compiler output files are new. Let the filesystem's
            // checked create establish absence and create atomically instead
            // of scanning the directory once here and a second time inside
            // ext4. A positive dentry remains the cheapest existing-file path.
            if self.ops.is_cacheable()
                && let Some(entry) = self.cache.read().children.get(name).cloned()
            {
                return Ok(entry);
            }

            let entry =
                match self.create_for_mutation(name, options.node_type, options.permission, false) {
                    Ok(entry) => entry,
                    Err(create_error) => {
                        // O_CREAT without O_EXCL opens a concurrently-created
                        // or previously uncached entry. If the name is still
                        // absent, preserve the checked-create failure (for
                        // example ENOSPC or EROFS).
                        return match self.lookup_fresh_for_mutation(name) {
                            Ok(entry) => Ok(entry),
                            Err(err) if err.canonicalize() == VfsError::NotFound => {
                                Err(create_error)
                            }
                            Err(err) => Err(err),
                        };
                    }
                };
            if options.user.is_some() {
                entry.update_metadata(MetadataUpdate {
                    owner: options.user,
                    ..Default::default()
                })?;
            }
            return Ok(entry);
        }

        let lookup_result = if options.create_new {
            self.lookup_fresh_for_mutation(name)
        } else {
            self.lookup_for_mutation(name)
        };
        match lookup_result {
            Ok(val) => {
                if options.create_new {
                    return Err(VfsError::AlreadyExists);
                }
                return Ok(val);
            }
            Err(err) if err.canonicalize() == VfsError::NotFound && options.create => {}
            Err(err) => return Err(err),
        }
        let entry =
            self.create_for_mutation(name, options.node_type, options.permission, true)?;
        if options.user.is_some() {
            entry.update_metadata(MetadataUpdate {
                owner: options.user,
                ..Default::default()
            })?;
        }
        Ok(entry)
    }

    pub fn mountpoint(&self) -> Option<Arc<Mountpoint>> {
        self.mountpoint.lock().clone()
    }

    pub fn is_mountpoint(&self) -> bool {
        self.mountpoint.lock().is_some()
    }

    /// Clears the cache of directory entries & user data, allowing them to be
    /// released.
    pub(crate) fn forget(&self) {
        let mut cache = self.cache.write();
        for (_, child) in mem::take(&mut cache.children) {
            if let Ok(dir) = child.as_dir() {
                dir.forget();
            }
        }
        cache.negative.clear();
        self.cache_generation.fetch_add(1, Ordering::AcqRel);
    }
}
