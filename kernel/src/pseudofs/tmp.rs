use alloc::{borrow::ToOwned, string::String, sync::Arc, vec::Vec};
use core::{any::Any, borrow::Borrow, cmp::Ordering, task::Context, time::Duration};

use axerrno::LinuxError;
use axfs_ng_vfs::{
    DeviceId, DirEntry, DirEntrySink, DirNode, DirNodeOps, FileNode, FileNodeOps, Filesystem,
    FilesystemOps, Metadata, MetadataUpdate, NodeFlags, NodeOps, NodePermission, NodeType,
    Reference, StatFs, VfsError, VfsResult, WeakDirEntry,
};
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
use hashbrown::HashMap;
use slab::Slab;

use crate::pseudofs::dummy_stat_fs;

#[derive(PartialEq, Eq, Hash, Clone)]
struct FileName(String);

impl PartialOrd for FileName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileName {
    fn cmp(&self, other: &Self) -> Ordering {
        fn index(s: &str) -> u8 {
            match s {
                "." => 0,
                ".." => 1,
                _ => 2,
            }
        }
        (index(&self.0), &self.0).cmp(&(index(&other.0), &other.0))
    }
}

impl<T> From<T> for FileName
where
    T: Into<String>,
{
    fn from(name: T) -> Self {
        Self(name.into())
    }
}

impl Borrow<str> for FileName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A simple in-memory filesystem that supports basic file operations.
pub struct MemoryFs {
    inodes: Mutex<Slab<Arc<Inode>>>,
    mount_flags: u32,
    root: Mutex<Option<DirEntry>>,
}

impl MemoryFs {
    /// Creates a new empty memory filesystem.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> Filesystem {
        Self::new_with_flags(0)
    }

    /// Creates a new empty memory filesystem with mount flags.
    #[allow(clippy::new_ret_no_self)]
    pub fn new_with_flags(mount_flags: u32) -> Filesystem {
        let fs = Arc::new(Self {
            inodes: Mutex::new(Slab::new()),
            mount_flags,
            root: Mutex::default(),
        });
        let root_ino = Inode::new(
            &fs,
            None,
            NodeType::Directory,
            NodePermission::from_bits_truncate(0o755),
        );
        *fs.root.lock() = Some(DirEntry::new_dir(
            |this| DirNode::new(MemoryNode::new(fs.clone(), root_ino, Some(this))),
            Reference::root(),
        ));
        Filesystem::new(fs)
    }

    fn get(&self, ino: u64) -> Arc<Inode> {
        self.inodes.lock()[ino as usize - 1].clone()
    }
}

impl FilesystemOps for MemoryFs {
    fn name(&self) -> &str {
        "tmpfs"
    }

    fn root_dir(&self) -> DirEntry {
        self.root.lock().clone().unwrap()
    }

    fn stat(&self) -> VfsResult<StatFs> {
        let mut stat = dummy_stat_fs(0x01021994);
        stat.mount_flags = self.mount_flags;
        Ok(stat)
    }
}

fn release_inode(fs: &MemoryFs, inode: &Arc<Inode>, nlink: u64) {
    let mut inodes = fs.inodes.lock();
    let mut metadata = inode.metadata.lock();
    metadata.nlink -= nlink;
    if metadata.nlink == 0 && Arc::strong_count(inode) == 2 {
        inodes.remove(metadata.inode as usize - 1);
    }
}

#[derive(Default)]
struct FileContent {
    /// The length of the file content.
    ///
    /// We only need to store the length here because we delegate the actual
    /// content management to page cache.
    length: Mutex<u64>,
    symlink: Mutex<Option<String>>,
}

#[derive(Default)]
struct DirContent {
    state: Mutex<DirState>,
}

#[derive(Default)]
struct DirState {
    entries: HashMap<FileName, InodeRef>,
    /// Monotonically increasing namespace version for this directory.
    generation: u64,
    /// Version at which each currently-present name acquired its inode.
    entry_versions: HashMap<FileName, u64>,
}

impl DirState {
    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn cache_version(&self, name: &str) -> u64 {
        self.entry_versions
            .get(name)
            .copied()
            .unwrap_or(self.generation)
    }

    fn mark_inserted(&mut self, name: &str) {
        let generation = self.next_generation();
        self.entry_versions
            .insert(name.to_owned().into(), generation);
    }

    fn mark_removed(&mut self, name: &str) {
        self.next_generation();
        self.entry_versions.remove(name);
    }

    fn mark_renamed(&mut self, src_name: &str, dst_name: &str) {
        let generation = self.next_generation();
        self.entry_versions.remove(src_name);
        self.entry_versions
            .insert(dst_name.to_owned().into(), generation);
    }
}

enum NodeContent {
    File(FileContent),
    Dir(DirContent),
}

struct Inode {
    ino: u64,
    metadata: Mutex<Metadata>,
    content: NodeContent,
    xattrs: Mutex<HashMap<Vec<u8>, Vec<u8>>>,
}

impl Inode {
    pub fn new(
        fs: &Arc<MemoryFs>,
        parent: Option<u64>,
        node_type: NodeType,
        permission: NodePermission,
    ) -> Arc<Inode> {
        let mut inodes = fs.inodes.lock();
        let entry = inodes.vacant_entry();
        let ino = entry.key() as u64 + 1;
        let metadata = Metadata {
            device: 0,
            inode: ino,
            nlink: 0,
            mode: permission,
            node_type,
            uid: 0,
            gid: 0,
            size: 0,
            block_size: 0,
            blocks: 0,
            rdev: DeviceId::default(),
            atime: Duration::default(),
            mtime: Duration::default(),
            ctime: Duration::default(),
        };
        let content = match node_type {
            NodeType::Directory => NodeContent::Dir(DirContent::default()),
            _ => NodeContent::File(FileContent::default()),
        };
        let result = Arc::new(Self {
            ino,
            metadata: Mutex::new(metadata),
            content,
            xattrs: Mutex::default(),
        });
        entry.insert(result.clone());
        drop(inodes);
        if let NodeContent::Dir(dir) = &result.content {
            let mut state = dir.state.lock();
            state
                .entries
                .insert(".".into(), InodeRef::new(fs.clone(), ino));
            state.entries.insert(
                "..".into(),
                InodeRef::new(fs.clone(), parent.unwrap_or(ino)),
            );
        }
        result
    }

    fn as_file(&self) -> VfsResult<&FileContent> {
        match self.content {
            NodeContent::File(ref content) => Ok(content),
            _ => Err(VfsError::IsADirectory),
        }
    }

    fn as_dir(&self) -> VfsResult<&DirContent> {
        match self.content {
            NodeContent::Dir(ref content) => Ok(content),
            _ => Err(VfsError::NotADirectory),
        }
    }
}

struct InodeRef {
    fs: Arc<MemoryFs>,
    ino: u64,
}

impl InodeRef {
    pub fn new(fs: Arc<MemoryFs>, ino: u64) -> Self {
        fs.get(ino).metadata.lock().nlink += 1;
        Self { fs, ino }
    }

    fn get(&self) -> Arc<Inode> {
        self.fs.get(self.ino)
    }
}

impl Drop for InodeRef {
    fn drop(&mut self) {
        release_inode(&self.fs, &self.get(), 1);
    }
}

struct MemoryNode {
    fs: Arc<MemoryFs>,
    inode: Arc<Inode>,
    this: Option<WeakDirEntry>,
}

impl MemoryNode {
    pub fn new(fs: Arc<MemoryFs>, inode: Arc<Inode>, this: Option<WeakDirEntry>) -> Arc<Self> {
        Arc::new(Self { fs, inode, this })
    }

    fn dirent_cookie(name: &str) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in name.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash.max(1)
    }

    fn new_entry(&self, name: &str, node_type: NodeType, inode: Arc<Inode>) -> VfsResult<DirEntry> {
        let fs = self.fs.clone();
        let reference = Reference::new(
            self.this.as_ref().and_then(WeakDirEntry::upgrade),
            name.to_owned(),
        );
        Ok(if node_type == NodeType::Directory {
            DirEntry::new_dir(
                |this| DirNode::new(MemoryNode::new(fs, inode, Some(this))),
                reference,
            )
        } else {
            DirEntry::new_file(
                FileNode::new(MemoryNode::new(fs, inode, None)),
                node_type,
                reference,
            )
        })
    }
}

impl NodeOps for MemoryNode {
    fn inode(&self) -> u64 {
        self.inode.ino
    }

    fn metadata(&self) -> VfsResult<Metadata> {
        let mut metadata = { self.inode.metadata.lock().clone() };
        match &self.inode.content {
            NodeContent::File(content) => {
                metadata.size = *content.length.lock();
            }
            NodeContent::Dir(dir) => {
                let state = dir.state.lock();
                metadata.size = state.entries.len() as u64;
                metadata.nlink = 2 + state
                    .entries
                    .iter()
                    .filter(|(name, entry)| {
                        name.0 != "."
                            && name.0 != ".."
                            && entry.get().metadata.lock().node_type == NodeType::Directory
                    })
                    .count() as u64;
            }
        }
        Ok(metadata)
    }

    fn update_metadata(&self, update: MetadataUpdate) -> VfsResult<()> {
        let mut metadata = self.inode.metadata.lock();
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
        Ok(())
    }

    fn filesystem(&self) -> &dyn FilesystemOps {
        self.fs.as_ref()
    }

    fn sync(&self, _data_only: bool) -> VfsResult<()> {
        Ok(())
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::ALWAYS_CACHE
    }

    fn set_xattr(&self, name: &[u8], value: &[u8]) -> VfsResult<()> {
        match self.inode.metadata.lock().node_type {
            NodeType::RegularFile | NodeType::Directory | NodeType::Symlink => {}
            _ => return Err(VfsError::OperationNotPermitted),
        }
        self.inode
            .xattrs
            .lock()
            .insert(name.to_vec(), value.to_vec());
        Ok(())
    }

    fn get_xattr(&self, name: &[u8], buf: &mut [u8]) -> VfsResult<usize> {
        let xattrs = self.inode.xattrs.lock();
        let value = xattrs
            .get(name)
            .ok_or(VfsError::from(LinuxError::ENODATA))?;
        let copy_len = buf.len().min(value.len());
        if copy_len != 0 {
            buf[..copy_len].copy_from_slice(&value[..copy_len]);
        }
        Ok(value.len())
    }

    fn list_xattr(&self) -> VfsResult<Vec<u8>> {
        let xattrs = self.inode.xattrs.lock();
        let mut names = Vec::new();
        for name in xattrs.keys() {
            names.extend_from_slice(name);
            names.push(0);
        }
        Ok(names)
    }

    fn remove_xattr(&self, name: &[u8]) -> VfsResult<()> {
        self.inode
            .xattrs
            .lock()
            .remove(name)
            .map(|_| ())
            .ok_or(VfsError::from(LinuxError::ENODATA))
    }
}

impl FileNodeOps for MemoryNode {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        let file = self.inode.as_file()?;
        if let Some(symlink) = file.symlink.lock().as_ref() {
            assert_eq!(offset, 0);
            let len = buf.len().min(symlink.len());
            buf[..len].copy_from_slice(&symlink.as_bytes()[..len]);
            return Ok(len);
        }
        unreachable!("page cache should handle reading");
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> VfsResult<usize> {
        unreachable!("page cache should handle writing");
    }

    fn append(&self, _buf: &[u8]) -> VfsResult<(usize, u64)> {
        unreachable!("page cache should handle writing");
    }

    fn set_len(&self, len: u64) -> VfsResult<()> {
        *self.inode.as_file()?.length.lock() = len;
        Ok(())
    }

    fn set_symlink(&self, target: &str) -> VfsResult<()> {
        let file = self.inode.as_file()?;
        *file.length.lock() = target.len() as u64;
        *file.symlink.lock() = Some(target.to_owned());
        Ok(())
    }
}
impl Pollable for MemoryNode {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

impl DirNodeOps for MemoryNode {
    fn read_dir(&self, offset: u64, sink: &mut dyn DirEntrySink) -> VfsResult<usize> {
        let mut entries = {
            let dir = self.inode.as_dir()?;
            let state = dir.state.lock();
            state
                .entries
                .iter()
                .map(|(name, entry)| {
                    let inode = entry.get();
                    let node_type = inode.metadata.lock().node_type;
                    (
                        Self::dirent_cookie(&name.0),
                        name.0.clone(),
                        entry.ino,
                        node_type,
                    )
                })
                .collect::<Vec<_>>()
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut count = 0;
        for (cookie, name, ino, node_type) in entries {
            if cookie <= offset {
                continue;
            }
            if !sink.accept(&name, ino, node_type, cookie) {
                return Ok(count);
            }
            count += 1;
        }
        Ok(count)
    }

    fn lookup(&self, name: &str) -> VfsResult<DirEntry> {
        let dir = self.inode.as_dir()?;
        let state = dir.state.lock();

        let entry = state.entries.get(name).ok_or(VfsError::NotFound)?;
        let inode = entry.get();
        let node_type = inode.metadata.lock().node_type;
        self.new_entry(name, node_type, inode)
    }

    fn cache_version(&self, name: &str) -> Option<u64> {
        self.inode
            .as_dir()
            .ok()
            .map(|dir| dir.state.lock().cache_version(name))
    }

    fn create(
        &self,
        name: &str,
        node_type: NodeType,
        permission: NodePermission,
    ) -> VfsResult<DirEntry> {
        let dir = self.inode.as_dir()?;
        let mut state = dir.state.lock();

        if state.entries.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }
        let inode = Inode::new(&self.fs, Some(self.inode.ino), node_type, permission);
        state
            .entries
            .insert(name.into(), InodeRef::new(self.fs.clone(), inode.ino));
        state.mark_inserted(name);
        self.new_entry(name, node_type, inode)
    }

    fn link(&self, name: &str, target: &DirEntry) -> VfsResult<DirEntry> {
        let dir = self.inode.as_dir()?;
        let mut state = dir.state.lock();

        let target = target.downcast::<Self>()?;

        if state.entries.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }
        let inode = target.inode.clone();
        let node_type = target.metadata()?.node_type;
        state
            .entries
            .insert(name.into(), InodeRef::new(self.fs.clone(), inode.ino));
        state.mark_inserted(name);
        self.new_entry(name, node_type, inode)
    }

    fn unlink(&self, name: &str) -> VfsResult<()> {
        let dir = self.inode.as_dir()?;
        let mut state = dir.state.lock();

        let Some(entry) = state.entries.get(name) else {
            return Err(VfsError::NotFound);
        };
        if let NodeContent::Dir(DirContent { state }) = &entry.get().content
            && state.lock().entries.len() > 2
        {
            return Err(VfsError::DirectoryNotEmpty);
        }
        state.entries.remove(name);
        state.mark_removed(name);

        Ok(())
    }

    fn rename(&self, src_name: &str, dst_dir: &DirNode, dst_name: &str) -> VfsResult<()> {
        let dst_node = dst_dir.downcast::<Self>()?;
        let src_dir = self.inode.as_dir()?;
        let dst_content = dst_node.inode.as_dir()?;

        if self.inode.ino == dst_node.inode.ino {
            let mut state = src_dir.state.lock();
            let src_ino = state.entries.get(src_name).ok_or(VfsError::NotFound)?.ino;
            if state
                .entries
                .get(dst_name)
                .is_some_and(|entry| entry.ino == src_ino)
            {
                return Ok(());
            }
            let src_entry = state.entries.remove(src_name).ok_or(VfsError::NotFound)?;
            state.entries.insert(dst_name.into(), src_entry);
            state.mark_renamed(src_name, dst_name);
            return Ok(());
        }

        // Lock two different directories in inode order so a cross-directory
        // rename is atomic with respect to tmpfs lookups and cannot deadlock
        // against a rename in the opposite direction.
        let (mut src_state, mut dst_state) = if self.inode.ino < dst_node.inode.ino {
            (src_dir.state.lock(), dst_content.state.lock())
        } else {
            let dst_state = dst_content.state.lock();
            let src_state = src_dir.state.lock();
            (src_state, dst_state)
        };
        let src_ino = src_state
            .entries
            .get(src_name)
            .ok_or(VfsError::NotFound)?
            .ino;
        if dst_state
            .entries
            .get(dst_name)
            .is_some_and(|entry| entry.ino == src_ino)
        {
            return Ok(());
        }
        let src_entry = src_state
            .entries
            .remove(src_name)
            .ok_or(VfsError::NotFound)?;
        dst_state.entries.insert(dst_name.into(), src_entry);
        src_state.mark_removed(src_name);
        dst_state.mark_inserted(dst_name);
        Ok(())
    }
}

impl Drop for MemoryNode {
    fn drop(&mut self) {
        // A MemoryNode is only a handle to an inode. Lookups may create
        // multiple handles for the same directory, so dropping an arbitrary
        // handle must not remove the directory's contents.
        //
        // An unlinked, empty directory has only its synthetic "." link left.
        // Reclaim "." and ".." when the last handle goes away; clearing them
        // earlier would make a still-linked directory appear empty.
        let reclaim_dir =
            self.inode.metadata.lock().nlink == 1 && Arc::strong_count(&self.inode) == 2;
        if reclaim_dir && let NodeContent::Dir(dir) = &self.inode.content {
            let mut state = dir.state.lock();
            state.entries.clear();
            state.entry_versions.clear();
        }
        release_inode(&self.fs, &self.inode, 0);
    }
}
