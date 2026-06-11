use alloc::{
    string::String,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::{
    iter, mem,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    task::Context,
    time::Duration,
};

use axpoll::{IoEvents, Pollable};
use hashbrown::HashMap;
use inherit_methods_macro::inherit_methods;

use crate::{
    DirEntry, DirEntrySink, Filesystem, FilesystemOps, Metadata, MetadataUpdate, Mutex, MutexGuard,
    NodeFlags, NodePermission, NodeType, OpenOptions, ReferenceKey, TypeMap, VfsError, VfsResult,
    path::{DOT, DOTDOT, PathBuf},
};

pub const MOUNT_RDONLY: u32 = 0x1;
pub const MOUNT_NOSYMFOLLOW: u32 = 0x100;

static DEVICE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct Mountpoint {
    /// Root dir entry in the mountpoint.
    root: DirEntry,
    /// Location in the parent mountpoint.
    location: Option<Location>,
    /// Children of the mountpoint.
    children: Mutex<HashMap<ReferenceKey, Weak<Self>>>,
    /// Device ID
    device: u64,
    /// Per-mount flags such as read-only and no-symlink-follow.
    mount_flags: AtomicU32,
}

impl Mountpoint {
    pub fn new(fs: &Filesystem, location_in_parent: Option<Location>) -> Arc<Self> {
        let root = fs.root_dir();
        let device = Self::next_device();
        let mount_flags = fs.stat().map_or(0, |stat| stat.mount_flags);
        Self::new_with_root(root, location_in_parent, device, mount_flags)
    }

    fn next_device() -> u64 {
        DEVICE_COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn new_with_root(
        root: DirEntry,
        location_in_parent: Option<Location>,
        device: u64,
        mount_flags: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            root,
            location: location_in_parent,
            children: Mutex::default(),
            device,
            mount_flags: AtomicU32::new(mount_flags),
        })
    }

    pub fn new_root(fs: &Filesystem) -> Arc<Self> {
        Self::new(fs, None)
    }

    pub fn root_location(self: &Arc<Self>) -> Location {
        Location::new(self.clone(), self.root.clone())
    }

    /// Returns the location in the parent mountpoint.
    pub fn location(&self) -> Option<Location> {
        self.location.clone()
    }

    pub fn is_root(&self) -> bool {
        self.location.is_none()
    }

    /// Returns the effective mountpoint.
    ///
    /// For example, first `mount /dev/sda1 /mnt` and then `mount /dev/sda2
    /// /mnt`. After the second mount is completed, the content of the first
    /// mount will be overridden (root mount -> mnt1 -> mnt2). We need to
    /// return `mnt2` for `mnt1.effective_mountpoint()`.
    pub(crate) fn effective_mountpoint(self: &Arc<Self>) -> Arc<Mountpoint> {
        let mut mountpoint = self.clone();
        while let Some(mount) = mountpoint.root.as_dir().unwrap().mountpoint() {
            if Arc::ptr_eq(&mount, &mountpoint) {
                break;
            }
            mountpoint = mount;
        }
        mountpoint
    }

    pub fn device(self: &Arc<Self>) -> u64 {
        self.device
    }

    pub fn mount_flags(&self) -> u32 {
        self.mount_flags.load(Ordering::Relaxed)
    }

    pub fn set_mount_flags(&self, flags: u32) {
        self.mount_flags.store(flags, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone)]
pub struct Location {
    mountpoint: Arc<Mountpoint>,
    entry: DirEntry,
}

#[inherit_methods(from = "self.entry")]
impl Location {
    pub fn inode(&self) -> u64;

    pub fn filesystem(&self) -> &dyn FilesystemOps;

    pub fn update_metadata(&self, update: MetadataUpdate) -> VfsResult<()>;

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> VfsResult<u64>;

    pub fn sync(&self, data_only: bool) -> VfsResult<()>;

    pub fn is_file(&self) -> bool;

    pub fn is_dir(&self) -> bool;

    pub fn node_type(&self) -> NodeType;

    pub fn read_link(&self) -> VfsResult<String>;

    pub fn ioctl(&self, cmd: u32, arg: usize) -> VfsResult<usize>;

    pub fn flags(&self) -> NodeFlags;

    pub fn set_xattr(&self, name: &[u8], value: &[u8]) -> VfsResult<()>;

    pub fn get_xattr(&self, name: &[u8], buf: &mut [u8]) -> VfsResult<usize>;

    pub fn list_xattr(&self) -> VfsResult<Vec<u8>>;

    pub fn remove_xattr(&self, name: &[u8]) -> VfsResult<()>;

    pub fn user_data(&self) -> MutexGuard<'_, TypeMap>;
}

impl Location {
    pub fn new(mountpoint: Arc<Mountpoint>, entry: DirEntry) -> Self {
        Self { mountpoint, entry }
    }

    fn wrap(&self, entry: DirEntry) -> Self {
        Self::new(self.mountpoint.clone(), entry)
    }

    pub fn mountpoint(&self) -> &Arc<Mountpoint> {
        &self.mountpoint
    }

    pub fn mount_flags(&self) -> u32 {
        self.mountpoint.mount_flags()
    }

    pub fn entry(&self) -> &DirEntry {
        &self.entry
    }

    pub fn name(&self) -> &str {
        if self.is_root_of_mount() {
            self.mountpoint.location.as_ref().map_or("", Location::name)
        } else {
            self.entry.name()
        }
    }

    pub fn parent(&self) -> Option<Self> {
        if !self.is_root_of_mount() {
            return Some(self.wrap(self.entry.parent().unwrap()));
        }
        self.mountpoint.location()?.parent()
    }

    pub fn is_root(&self) -> bool {
        self.mountpoint.is_root() && self.entry.is_root_of_mount()
    }

    pub fn is_root_of_mount(&self) -> bool {
        self.entry.ptr_eq(&self.mountpoint.root)
    }

    pub fn check_is_dir(&self) -> VfsResult<()> {
        self.entry.as_dir().map(|_| ())
    }

    pub fn check_is_file(&self) -> VfsResult<()> {
        self.entry.as_file().map(|_| ())
    }

    pub fn metadata(&self) -> VfsResult<Metadata> {
        let mut metadata = self.entry.metadata()?;
        metadata.device = self.mountpoint.device();
        Ok(metadata)
    }

    pub fn cached_metadata(&self) -> Option<Metadata> {
        self.entry.cached_metadata().map(|mut metadata| {
            metadata.device = self.mountpoint.device();
            metadata
        })
    }

    pub fn grow_cached_len(&self, len: u64, dirty: bool) -> VfsResult<u64> {
        self.entry.grow_cached_len(len, dirty)
    }

    pub fn set_cached_len(&self, len: u64, dirty: bool) -> VfsResult<()> {
        self.entry.set_cached_len(len, dirty)
    }

    pub fn set_cached_len_if_present(&self, len: u64) {
        self.entry.set_cached_len_if_present(len)
    }

    pub fn grow_cached_len_if_present(&self, len: u64) {
        self.entry.grow_cached_len_if_present(len)
    }

    pub fn record_access(&self, atime: Duration, strict: bool) -> VfsResult<bool> {
        self.entry.record_access(atime, strict)
    }

    pub fn record_modification(&self, mtime: Duration) -> VfsResult<bool> {
        self.entry.record_modification(mtime)
    }

    pub fn flush_metadata_cache(&self, data_only: bool) -> VfsResult<()> {
        self.entry.flush_metadata_cache(data_only)
    }

    pub fn invalidate_metadata_cache(&self) {
        self.entry.invalidate_metadata_cache()
    }

    pub fn absolute_path(&self) -> VfsResult<PathBuf> {
        let mut components = vec![];
        let mut cur = self.clone();
        loop {
            let mut entry = cur.entry.clone();
            while !entry.ptr_eq(&cur.mountpoint.root) {
                components.push(String::from(entry.name()));
                let Some(parent) = entry.parent() else {
                    break;
                };
                entry = parent;
            }
            cur = match cur.mountpoint.location() {
                Some(loc) => loc,
                None => break,
            }
        }
        Ok(iter::once("/")
            .chain(components.iter().map(String::as_str).rev())
            .collect())
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.mountpoint, &other.mountpoint) && self.entry.ptr_eq(&other.entry)
    }

    pub fn is_mountpoint(&self) -> bool {
        self.entry.as_dir().is_ok_and(|it| it.is_mountpoint())
    }

    /// See [`Mountpoint::effective_mountpoint`].
    fn resolve_mountpoint(self) -> Self {
        let Some(mountpoint) = self.entry.as_dir().ok().and_then(|it| it.mountpoint()) else {
            return self;
        };
        let mountpoint = mountpoint.effective_mountpoint();
        let entry = mountpoint.root.clone();
        Self::new(mountpoint, entry)
    }

    pub fn lookup_no_follow(&self, name: &str) -> VfsResult<Self> {
        Ok(match name {
            DOT => self.clone(),
            DOTDOT => self.parent().unwrap_or_else(|| self.clone()),
            _ => {
                let loc = Self::new(self.mountpoint.clone(), self.entry.as_dir()?.lookup(name)?);
                loc.resolve_mountpoint()
            }
        })
    }

    pub fn create(
        &self,
        name: &str,
        node_type: NodeType,
        permission: NodePermission,
    ) -> VfsResult<Self> {
        self.entry
            .as_dir()?
            .create(name, node_type, permission)
            .map(|entry| self.wrap(entry))
    }

    pub fn link(&self, name: &str, node: &Self) -> VfsResult<Self> {
        if !Arc::ptr_eq(&self.mountpoint, &node.mountpoint) {
            return Err(VfsError::CrossesDevices);
        }
        self.entry
            .as_dir()?
            .link(name, &node.entry)
            .map(|entry| self.wrap(entry))
    }

    pub fn rename(&self, src_name: &str, dst_dir: &Self, dst_name: &str) -> VfsResult<()> {
        if !Arc::ptr_eq(&self.mountpoint, &dst_dir.mountpoint) {
            return Err(VfsError::CrossesDevices);
        }
        let src_entry = self.entry.as_dir()?.lookup(src_name)?;
        if src_entry.node_type() == NodeType::Directory
            && !self.ptr_eq(dst_dir)
            && src_entry.is_ancestor_of(&dst_dir.entry)?
        {
            return Err(VfsError::InvalidInput);
        }
        self.entry
            .as_dir()?
            .rename(src_name, dst_dir.entry.as_dir()?, dst_name)
    }

    pub fn unlink(&self, name: &str, is_dir: bool) -> VfsResult<()> {
        self.entry.as_dir()?.unlink(name, is_dir)
    }

    pub fn open_file(&self, name: &str, options: &OpenOptions) -> VfsResult<Location> {
        self.entry
            .as_dir()?
            .open_file(name, options)
            .map(|entry| self.wrap(entry).resolve_mountpoint())
    }

    pub fn read_dir(&self, offset: u64, sink: &mut dyn DirEntrySink) -> VfsResult<usize> {
        self.entry.as_dir()?.read_dir(offset, sink)
    }

    pub fn mount(&self, fs: &Filesystem) -> VfsResult<Arc<Mountpoint>> {
        self.mount_with_flags(fs, fs.stat().map_or(0, |stat| stat.mount_flags))
    }

    pub fn mount_with_flags(&self, fs: &Filesystem, flags: u32) -> VfsResult<Arc<Mountpoint>> {
        let mut mountpoint = self.entry.as_dir()?.mountpoint.lock();
        if mountpoint.is_some() {
            return Err(VfsError::ResourceBusy);
        }
        let result = Mountpoint::new_with_root(
            fs.root_dir(),
            Some(self.clone()),
            Mountpoint::next_device(),
            flags,
        );
        *mountpoint = Some(result.clone());
        self.mountpoint
            .children
            .lock()
            .insert(self.entry.key(), Arc::downgrade(&result));
        Ok(result)
    }

    pub fn bind_mount(&self, source: &Self) -> VfsResult<Arc<Mountpoint>> {
        source.check_is_dir()?;
        let mut mountpoint = self.entry.as_dir()?.mountpoint.lock();
        if mountpoint.is_some() {
            return Err(VfsError::ResourceBusy);
        }
        let result = Mountpoint::new_with_root(
            source.entry.clone(),
            Some(self.clone()),
            source.mountpoint.device(),
            source.mountpoint.mount_flags(),
        );
        *mountpoint = Some(result.clone());
        self.mountpoint
            .children
            .lock()
            .insert(self.entry.key(), Arc::downgrade(&result));
        Ok(result)
    }

    pub fn unmount(&self) -> VfsResult<()> {
        if !self.is_root_of_mount() {
            return Err(VfsError::InvalidInput);
        }
        let mut children = self.mountpoint.children.lock();
        children.retain(|_, child| child.strong_count() > 0);
        if !children.is_empty() {
            return Err(VfsError::ResourceBusy);
        }
        drop(children);
        assert!(self.entry.ptr_eq(&self.mountpoint.root));
        self.entry.as_dir()?.forget();
        if let Some(parent_loc) = &self.mountpoint.location {
            parent_loc
                .mountpoint
                .children
                .lock()
                .remove(&parent_loc.entry.key());
            *parent_loc.entry.as_dir()?.mountpoint.lock() = None;
        }
        Ok(())
    }

    pub fn unmount_all(&self) -> VfsResult<()> {
        if !self.is_root_of_mount() {
            return Err(VfsError::InvalidInput);
        }
        let children = mem::take(&mut *self.mountpoint.children.lock());
        for (_, child) in children {
            if let Some(child) = child.upgrade() {
                child.root_location().unmount_all()?;
            }
        }
        self.unmount()
    }
}

#[inherit_methods(from = "self.entry")]
impl Pollable for Location {
    fn poll(&self) -> IoEvents;

    fn register(&self, context: &mut Context<'_>, events: IoEvents);
}
