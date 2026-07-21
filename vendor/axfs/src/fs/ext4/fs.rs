use alloc::sync::Arc;
use core::{
    cell::OnceCell,
    ops::{Deref, DerefMut},
};

use axdriver::AxBlockDevice;
use axfs_ng_vfs::{
    DirEntry, DirNode, Filesystem, FilesystemOps, Reference, StatFs, VfsResult, path::MAX_NAME_LEN,
};
use axsync::{Mutex, MutexGuard};
use lwext4_rust::{FsConfig, ffi::EXT4_ROOT_INO};

use super::{
    Ext4Disk, Inode,
    util::{LwExt4Filesystem, into_vfs_err},
};

const EXT4_CONFIG: FsConfig = FsConfig { bcache_size: 4096 };

pub(crate) struct Ext4Guard<'a> {
    inner: MutexGuard<'a, LwExt4Filesystem>,
    #[cfg(feature = "profile")]
    _profile: axsync::ProfileScope,
}

impl Deref for Ext4Guard<'_> {
    type Target = LwExt4Filesystem;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Ext4Guard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub struct Ext4Filesystem {
    inner: Mutex<LwExt4Filesystem>,
    root_dir: OnceCell<DirEntry>,
}

impl Ext4Filesystem {
    pub fn new(dev: AxBlockDevice) -> VfsResult<Filesystem> {
        Self::new_disk(Ext4Disk::runtime(dev))
    }

    pub fn new_driver(
        dev: impl axdriver::prelude::BlockDriverOps + 'static,
    ) -> VfsResult<Filesystem> {
        Self::new_disk(Ext4Disk::driver(dev))
    }

    fn new_disk(disk: Ext4Disk) -> VfsResult<Filesystem> {
        let ext4 = lwext4_rust::Ext4Filesystem::new(disk, EXT4_CONFIG).map_err(into_vfs_err)?;
        let fs = Arc::new(Self {
            inner: Mutex::new(ext4),
            root_dir: OnceCell::new(),
        });
        let _ = fs.root_dir.set(DirEntry::new_dir(
            |this| DirNode::new(Inode::new(fs.clone(), EXT4_ROOT_INO, Some(this))),
            Reference::root(),
        ));
        Ok(Filesystem::new(fs))
    }

    pub(crate) fn lock(&self) -> Ext4Guard<'_> {
        #[cfg(feature = "profile")]
        let profile =
            axsync::ProfileScope::new(axsync::ProfileEvent::Ext4, self as *const Self as usize);

        #[cfg(feature = "profile")]
        let inner = if let Some(inner) = self.inner.try_lock() {
            inner
        } else {
            let wait = axsync::ProfileScope::new(
                axsync::ProfileEvent::Ext4LockWait,
                &self.inner as *const Mutex<LwExt4Filesystem> as usize,
            );
            let inner = self.inner.lock();
            drop(wait);
            inner
        };
        #[cfg(not(feature = "profile"))]
        let inner = self.inner.lock();

        Ext4Guard {
            inner,
            #[cfg(feature = "profile")]
            _profile: profile,
        }
    }
}

unsafe impl Send for Ext4Filesystem {}

unsafe impl Sync for Ext4Filesystem {}

impl FilesystemOps for Ext4Filesystem {
    fn name(&self) -> &str {
        "ext4"
    }

    fn root_dir(&self) -> DirEntry {
        self.root_dir.get().unwrap().clone()
    }

    fn stat(&self) -> VfsResult<StatFs> {
        let mut fs = self.lock();
        let stat = fs.stat().map_err(into_vfs_err)?;
        Ok(StatFs {
            fs_type: 0xef53,
            block_size: stat.block_size as _,
            blocks: stat.blocks_count,
            blocks_free: stat.free_blocks_count,
            blocks_available: stat.free_blocks_count,

            file_count: stat.inodes_count as _,
            free_file_count: stat.free_inodes_count as _,

            name_length: MAX_NAME_LEN as _,
            fragment_size: 0,
            mount_flags: 0,
        })
    }

    fn flush(&self) -> VfsResult<()> {
        self.lock().flush().map_err(into_vfs_err)
    }
}
