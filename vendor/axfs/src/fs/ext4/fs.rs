use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    cell::OnceCell,
    ops::{Deref, DerefMut},
};

use axdriver::AxBlockDevice;
use axfs_ng_vfs::{
    DirEntry, DirNode, Filesystem, FilesystemOps, Reference, StatFs, VfsResult, path::MAX_NAME_LEN,
};
use axsync::{Mutex, MutexGuard};
use lwext4_rust::{BlockExtent, FsConfig, ffi::EXT4_ROOT_INO};
use spin::RwLock;

use super::{
    Ext4Disk, Inode,
    block_io::SharedBlockDevice,
    util::{LwExt4Filesystem, into_vfs_err},
};

const EXT4_CONFIG: FsConfig = FsConfig { bcache_size: 4096 };
const EXTENT_CACHE_SHARDS: usize = 64;
const EXTENT_CACHE_LOOKAHEAD_BYTES: usize = 1024 * 1024;

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
    disk: SharedBlockDevice,
    block_size: usize,
    extent_cache: [RwLock<BTreeMap<u32, BTreeMap<u32, BlockExtent>>>; EXTENT_CACHE_SHARDS],
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
        let shared_disk = disk.shared();
        let ext4 = lwext4_rust::Ext4Filesystem::new(disk, EXT4_CONFIG).map_err(into_vfs_err)?;
        let block_size = ext4.block_size() as usize;
        let fs = Arc::new(Self {
            inner: Mutex::new(ext4),
            disk: shared_disk,
            block_size,
            extent_cache: [const { RwLock::new(BTreeMap::new()) }; EXTENT_CACHE_SHARDS],
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

    fn extent_cache_for(
        &self,
        ino: u32,
    ) -> &RwLock<BTreeMap<u32, BTreeMap<u32, BlockExtent>>> {
        &self.extent_cache[ino as usize % EXTENT_CACHE_SHARDS]
    }

    fn cached_extents(&self, ino: u32, start: u32, count: u32) -> Option<Vec<BlockExtent>> {
        let end = start.checked_add(count)?;
        let cache = self.extent_cache_for(ino).read();
        let inode = cache.get(&ino)?;
        let mut result = Vec::new();
        let mut logical = start;

        while logical < end {
            let extent = inode.range(..=logical).next_back()?.1;
            let extent_end = extent.logical_block.checked_add(extent.block_count)?;
            if logical < extent.logical_block || logical >= extent_end {
                return None;
            }
            let block_count = (extent_end - logical).min(end - logical);
            let physical_block = extent
                .physical_block
                .map(|physical| physical + u64::from(logical - extent.logical_block));
            result.push(BlockExtent {
                logical_block: logical,
                physical_block,
                block_count,
            });
            logical += block_count;
        }
        Some(result)
    }

    fn cache_extents(&self, ino: u32, extents: &[BlockExtent]) {
        if extents.is_empty() {
            return;
        }
        let mut cache = self.extent_cache_for(ino).write();
        let inode = cache.entry(ino).or_default();
        for extent in extents {
            let start = extent.logical_block;
            let end = start.saturating_add(extent.block_count);
            let overlaps = inode
                .range(..end)
                .filter(|(_, cached)| {
                    cached.logical_block.saturating_add(cached.block_count) > start
                })
                .map(|(logical, _)| *logical)
                .collect::<Vec<_>>();
            for logical in overlaps {
                inode.remove(&logical);
            }
            inode.insert(start, *extent);
        }
    }

    pub(crate) fn invalidate_extent_cache(&self, ino: u32) {
        self.extent_cache_for(ino).write().remove(&ino);
    }

    fn read_extents(
        &self,
        ino: u32,
        start: u32,
        count: u32,
        file_size: u64,
    ) -> VfsResult<Vec<BlockExtent>> {
        if let Some(extents) = self.cached_extents(ino, start, count) {
            return Ok(extents);
        }

        let file_blocks = file_size.div_ceil(self.block_size as u64);
        let available = file_blocks
            .saturating_sub(u64::from(start))
            .min(u64::from(u32::MAX)) as u32;
        let lookahead = (EXTENT_CACHE_LOOKAHEAD_BYTES / self.block_size)
            .max(1)
            .min(u32::MAX as usize) as u32;
        let mapping_count = count.max(lookahead.min(available));
        let extents = {
            let mut fs = self.lock();
            // Another task may have populated the lookahead window while this
            // task slept on the ext4 metadata lock. Recheck after acquiring
            // the lock so concurrent faults in one file do not repeat the
            // same block-map walk serially.
            if let Some(extents) = self.cached_extents(ino, start, count) {
                return Ok(extents);
            }
            let extents = fs.map_blocks(ino, start, mapping_count).map_err(|err| {
                error!(
                    "ext4 map failed: ino={ino} logical={start} count={mapping_count} error={err:?}"
                );
                into_vfs_err(err)
            })?;
            // Once these blocks leave lwext4's buffer cache, regular-file
            // data uses only the address_space and direct block path. Any
            // operation that may change data or allocation invalidates this
            // inode's extent cache before entering lwext4 again.
            fs.prepare_direct_data_access(&extents).map_err(|err| {
                error!(
                    "ext4 direct-data preparation failed: ino={ino} logical={start} count={mapping_count} error={err:?}"
                );
                into_vfs_err(err)
            })?;
            extents
        };
        self.cache_extents(ino, &extents);
        self.cached_extents(ino, start, count)
            .ok_or(axfs_ng_vfs::VfsError::Io)
    }

    /// Reads naturally aligned file blocks without retaining the lwext4
    /// metadata lock while the block device services the request.
    pub(crate) fn read_mapped(
        &self,
        ino: u32,
        buf: &mut [u8],
        offset: u64,
        file_size: u64,
    ) -> VfsResult<usize> {
        let block_size = self.block_size;
        if offset % block_size as u64 != 0 || buf.len() % block_size != 0 {
            return self
                .lock()
                .read_at(ino, buf, offset)
                .map_err(into_vfs_err);
        }
        if offset >= file_size || buf.is_empty() {
            return Ok(0);
        }
        let valid = (file_size - offset).min(buf.len() as u64) as usize;
        let block_count = u32::try_from(valid.div_ceil(block_size))
            .map_err(|_| axfs_ng_vfs::VfsError::InvalidInput)?;
        let logical_block = u32::try_from(offset / block_size as u64)
            .map_err(|_| axfs_ng_vfs::VfsError::InvalidInput)?;
        let extents = self.read_extents(ino, logical_block, block_count, file_size)?;

        let valid = (file_size - offset).min(buf.len() as u64) as usize;
        let device_block_size = self.disk.block_size();
        if block_size % device_block_size != 0 {
            return Err(axfs_ng_vfs::VfsError::OperationNotSupported);
        }
        let sectors_per_block = block_size / device_block_size;

        for extent in extents {
            let first =
                (extent.logical_block as u64 * block_size as u64).saturating_sub(offset) as usize;
            let len = extent.block_count as usize * block_size;
            let data = &mut buf[first..first + len];
            if let Some(physical_block) = extent.physical_block {
                let sector = physical_block * sectors_per_block as u64;
                self.disk.read_blocks(sector, data).map_err(|err| {
                    error!(
                        "ext4 direct read failed: ino={ino} logical={} sector={sector} bytes={} error={err:?}",
                        extent.logical_block,
                        data.len()
                    );
                    axfs_ng_vfs::VfsError::Io
                })?;
            } else {
                data.fill(0);
            }
        }
        buf[valid..].fill(0);
        Ok(valid)
    }

    /// Allocates file-block mappings while holding lwext4's metadata lock, then
    /// releases it before submitting the data transfer.
    pub(crate) fn write_mapped(
        &self,
        ino: u32,
        buf: &[u8],
        offset: u64,
        file_len: u64,
    ) -> VfsResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.invalidate_extent_cache(ino);
        let write_end = offset
            .checked_add(buf.len() as u64)
            .ok_or(axfs_ng_vfs::VfsError::InvalidInput)?;

        let (block_size, extents) = {
            let mut fs = self.lock();
            let block_size = self.block_size;
            if !offset.is_multiple_of(block_size as u64)
                || (!buf.len().is_multiple_of(block_size) && write_end < file_len)
            {
                // A partial block before EOF needs lwext4's read-modify-write
                // path. Zero-padding is safe only for the final block at EOF.
                return fs.write_at(ino, buf, offset).map_err(into_vfs_err);
            }

            let logical_block = u32::try_from(offset / block_size as u64)
                .map_err(|_| axfs_ng_vfs::VfsError::InvalidInput)?;
            let block_count = u32::try_from(buf.len().div_ceil(block_size))
                .map_err(|_| axfs_ng_vfs::VfsError::InvalidInput)?;
            let extents = match fs.map_blocks_for_write(ino, logical_block, block_count) {
                Ok(extents) => extents,
                Err(_) => return fs.write_at(ino, buf, offset).map_err(into_vfs_err),
            };
            fs.prepare_direct_data_access(&extents)
                .map_err(into_vfs_err)?;
            (block_size, extents)
        };

        let mut padded = Vec::new();
        let io: &[u8] = if buf.len().is_multiple_of(block_size) {
            buf
        } else {
            padded.extend_from_slice(buf);
            padded.resize(buf.len().div_ceil(block_size) * block_size, 0);
            &padded
        };

        let device_block_size = self.disk.block_size();
        if !block_size.is_multiple_of(device_block_size) {
            return Err(axfs_ng_vfs::VfsError::OperationNotSupported);
        }
        let sectors_per_block = block_size / device_block_size;
        for extent in &extents {
            let first = (extent.logical_block as u64 * block_size as u64 - offset) as usize;
            let len = extent.block_count as usize * block_size;
            let physical_block = extent.physical_block.ok_or(axfs_ng_vfs::VfsError::Io)?;
            self.disk
                .write_blocks(
                    physical_block * sectors_per_block as u64,
                    &io[first..first + len],
                )
                .map_err(|_| axfs_ng_vfs::VfsError::Io)?;
        }

        // CachedFile commits the final inode length once after the whole
        // writeback batch. Reacquiring the global lwext4 metadata lock after
        // every folio would serialize otherwise independent data I/O.
        self.cache_extents(ino, &extents);
        Ok(buf.len())
    }

    pub(crate) fn flush_all(&self) -> VfsResult<()> {
        self.lock().flush().map_err(into_vfs_err)?;
        self.disk.flush().map_err(|_| axfs_ng_vfs::VfsError::Io)
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
        self.flush_all()
    }
}
