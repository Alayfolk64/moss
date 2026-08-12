use alloc::sync::Arc;

use inherit_methods_macro::inherit_methods;

use crate::{DirEntry, VfsResult};

pub struct StatFs {
    pub fs_type: u32,
    pub block_size: u32,
    pub blocks: u64,
    pub blocks_free: u64,      // 空闲块数
    pub blocks_available: u64, // 可用块数

    pub file_count: u64,
    pub free_file_count: u64, // 可创建的文件数

    pub name_length: u32,
    pub fragment_size: u32,
    pub mount_flags: u32,
}

/// Trait for filesystem operations
pub trait FilesystemOps: Send + Sync {
    /// Gets the name of the filesystem
    fn name(&self) -> &str;

    /// Gets the root directory entry of the filesystem
    fn root_dir(&self) -> DirEntry;

    /// Returns statistics about the filesystem
    fn stat(&self) -> VfsResult<StatFs>;

    /// Flushes the filesystem, ensuring all data is written to disk
    fn flush(&self) -> VfsResult<()> {
        Ok(())
    }

    /// Finalizes a filesystem before its block device is handed to another
    /// operating system or the machine is reset.
    ///
    /// Most in-memory filesystems need no work beyond a flush.  On-disk
    /// filesystems can override this hook to update their superblock's clean
    /// state after all mounts have been detached.
    fn shutdown(&self) -> VfsResult<()> {
        self.flush()
    }
}

#[derive(Clone)]
pub struct Filesystem {
    ops: Arc<dyn FilesystemOps>,
}

#[inherit_methods(from = "self.ops")]
impl Filesystem {
    pub fn name(&self) -> &str;

    pub fn root_dir(&self) -> DirEntry;

    pub fn stat(&self) -> VfsResult<StatFs>;
}

impl Filesystem {
    pub fn new(ops: Arc<dyn FilesystemOps>) -> Self {
        Self { ops }
    }
}
