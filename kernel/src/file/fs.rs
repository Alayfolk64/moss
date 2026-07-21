use alloc::{
    borrow::Cow,
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
};
use core::{
    ffi::c_int,
    hint::likely,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::Context,
    time::Duration,
};

use axerrno::{AxError, AxResult};
use axfs::{FS_CONTEXT, FileFlags, FsContext, cached_metadata, update_cached_metadata};
use axfs_ng_vfs::{Location, Metadata, MetadataUpdate, NodeFlags};
use axhal::time::realtime_time;
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
use axtask::future::{block_on, poll_io};
use linux_raw_sys::general::{
    AT_EMPTY_PATH, AT_FDCWD, AT_SYMLINK_NOFOLLOW, FS_APPEND_FL, FS_COMPR_FL, FS_IMMUTABLE_FL,
    FS_NODUMP_FL, MS_NOATIME, MS_NODIRATIME, MS_STRICTATIME, O_APPEND, O_NOATIME, O_NONBLOCK,
    O_PATH, O_RDONLY, O_RDWR, O_WRONLY, STATX_ATTR_MOUNT_ROOT,
};
use spin::RwLock;

use super::{FileLike, Kstat, get_file_like};
use crate::file::{IoDst, IoSrc};

#[derive(Clone, Copy)]
struct SpecialNodeMetadata {
    mode: u32,
    rdev: axfs_ng_vfs::DeviceId,
}

static SPECIAL_NODES: RwLock<BTreeMap<String, SpecialNodeMetadata>> = RwLock::new(BTreeMap::new());
static INODE_FLAGS: RwLock<BTreeMap<String, u32>> = RwLock::new(BTreeMap::new());
const PATH_MAX: usize = 4096;

pub fn register_special_node(path: String, mode: u32, rdev: axfs_ng_vfs::DeviceId) {
    SPECIAL_NODES
        .write()
        .insert(path, SpecialNodeMetadata { mode, rdev });
}

pub fn inode_flags(path: &str) -> u32 {
    *INODE_FLAGS.read().get(path).unwrap_or(&0)
}

pub fn set_inode_flags(path: String, flags: u32) {
    const SUPPORTED: u32 = FS_COMPR_FL | FS_IMMUTABLE_FL | FS_APPEND_FL | FS_NODUMP_FL;
    INODE_FLAGS.write().insert(path, flags & SUPPORTED);
}

fn should_update_atime_for_access(loc: &Location, is_dir: bool, now: Duration) -> bool {
    let flags = loc.mount_flags();
    if flags & MS_NOATIME != 0 || (is_dir && flags & MS_NODIRATIME != 0) {
        return false;
    }
    if flags & MS_STRICTATIME == 0 {
        let Ok(metadata) = cached_metadata(loc) else {
            return false;
        };
        let stale_by_time = now.checked_sub(metadata.atime).map_or(false, |elapsed| {
            elapsed >= Duration::from_secs(24 * 60 * 60)
        });
        if metadata.atime > metadata.mtime && metadata.atime > metadata.ctime && !stale_by_time {
            return false;
        }
    }
    true
}

pub fn update_atime_for_access(loc: &Location, is_dir: bool) {
    let now = realtime_time();
    if !should_update_atime_for_access(loc, is_dir, now) {
        return;
    }
    let _ = update_cached_metadata(
        loc,
        MetadataUpdate {
            atime: Some(now),
            ..Default::default()
        },
    );
}

fn record_file_access(file: &axfs::File) {
    let loc = file.location();
    let now = realtime_time();
    if should_update_atime_for_access(loc, false, now) {
        let _ = update_cached_metadata(
            loc,
            MetadataUpdate {
                atime: Some(now),
                ..Default::default()
            },
        );
    }
}

pub fn with_fs<R>(dirfd: c_int, f: impl FnOnce(&mut FsContext) -> AxResult<R>) -> AxResult<R> {
    let mut fs = FS_CONTEXT.lock();
    if dirfd == AT_FDCWD {
        f(&mut fs)
    } else {
        let dir = Directory::from_fd(dirfd)?.inner.clone();
        f(&mut fs.with_current_dir(dir)?)
    }
}

pub enum ResolveAtResult {
    File(Location),
    Other(Arc<dyn FileLike>),
}

impl ResolveAtResult {
    pub fn into_file(self) -> Option<Location> {
        match self {
            Self::File(file) => Some(file),
            Self::Other(_) => None,
        }
    }

    pub fn stat(&self) -> AxResult<Kstat> {
        match self {
            Self::File(file) => {
                let mut stat = cached_metadata(file).map(|it| metadata_to_kstat(&it))?;
                if let Ok(path) = file.absolute_path()
                    && let Some(special) = SPECIAL_NODES.read().get(path.as_str()).copied()
                {
                    stat.mode = special.mode;
                    stat.rdev = special.rdev;
                }
                if let Ok(path) = file.absolute_path() {
                    let path = path.as_str();
                    stat.attributes |= inode_flags(path) as u64;
                    if file.is_root_of_mount() {
                        stat.attributes |= STATX_ATTR_MOUNT_ROOT as u64;
                    }
                }
                Ok(stat)
            }
            Self::Other(file_like) => file_like.stat(),
        }
    }
}

pub fn resolve_at(dirfd: c_int, path: Option<&str>, flags: u32) -> AxResult<ResolveAtResult> {
    match path {
        Some("") | None => {
            if flags & AT_EMPTY_PATH == 0 {
                return Err(AxError::NotFound);
            }
            if dirfd == AT_FDCWD {
                return Ok(ResolveAtResult::File(
                    FS_CONTEXT.lock().current_dir().clone(),
                ));
            }
            let file_like = get_file_like(dirfd)?;
            let f = file_like.clone();
            Ok(if let Some(file) = f.downcast_ref::<File>() {
                ResolveAtResult::File(file.inner().location().clone())
            } else if let Some(dir) = f.downcast_ref::<Directory>() {
                ResolveAtResult::File(dir.inner().clone())
            } else {
                ResolveAtResult::Other(file_like)
            })
        }
        Some(path) => {
            if path.len() >= PATH_MAX {
                return Err(AxError::NameTooLong);
            }
            let dirfd = if path.starts_with('/') {
                AT_FDCWD
            } else {
                dirfd
            };
            with_fs(dirfd, |fs| {
                if flags & AT_SYMLINK_NOFOLLOW != 0 {
                    fs.resolve_no_follow(path)
                } else {
                    fs.resolve(path)
                }
                .map(ResolveAtResult::File)
            })
        }
    }
}

pub fn metadata_to_kstat(metadata: &Metadata) -> Kstat {
    let ty = metadata.node_type as u8;
    let perm = metadata.mode.bits() as u32;
    let mode = ((ty as u32) << 12) | perm;
    let blksize = if metadata.block_size == 0 {
        4096
    } else {
        metadata.block_size
    };
    Kstat {
        dev: metadata.device,
        ino: metadata.inode,
        mode,
        nlink: metadata.nlink as _,
        uid: metadata.uid,
        gid: metadata.gid,
        size: metadata.size,
        blksize: blksize as _,
        blocks: metadata.blocks,
        rdev: metadata.rdev,
        attributes: 0,
        attributes_mask: 0,
        atime: metadata.atime,
        btime: metadata.ctime,
        mtime: metadata.mtime,
        ctime: metadata.ctime,
    }
}

/// File wrapper for `axfs::fops::File`.
pub struct File {
    inner: axfs::File,
    nonblock: AtomicBool,
    status_flags: AtomicU32,
    write_mount_id: Option<usize>,
}

static MOUNT_WRITE_OPEN_COUNTS: RwLock<BTreeMap<usize, usize>> = RwLock::new(BTreeMap::new());

fn mount_id(loc: &Location) -> usize {
    Arc::as_ptr(loc.mountpoint()) as usize
}

fn note_mount_write_open(id: usize) {
    let mut counts = MOUNT_WRITE_OPEN_COUNTS.write();
    *counts.entry(id).or_insert(0) += 1;
}

fn note_mount_write_close(id: usize) {
    let mut counts = MOUNT_WRITE_OPEN_COUNTS.write();
    if let Some(count) = counts.get_mut(&id) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&id);
        }
    }
}

pub fn mount_write_open_count(loc: &Location) -> usize {
    MOUNT_WRITE_OPEN_COUNTS
        .read()
        .get(&mount_id(loc))
        .copied()
        .unwrap_or(0)
}

impl File {
    fn open_mode_from_flags(flags: FileFlags) -> u32 {
        match (
            flags.contains(FileFlags::READ),
            flags.contains(FileFlags::WRITE),
        ) {
            (true, true) => O_RDWR,
            (false, true) => O_WRONLY,
            _ => O_RDONLY,
        }
    }

    pub fn new(inner: axfs::File) -> Self {
        Self::new_with_status_flags(inner, 0)
    }

    pub fn new_with_status_flags(inner: axfs::File, open_flags: u32) -> Self {
        let mut status_flags = Self::open_mode_from_flags(inner.flags());
        if inner.flags().contains(FileFlags::APPEND) {
            status_flags |= O_APPEND;
        }
        status_flags |= open_flags & (O_NOATIME | O_NONBLOCK | O_PATH);
        let write_mount_id = (inner.flags().contains(FileFlags::WRITE)
            && !inner.flags().contains(FileFlags::PATH))
        .then(|| mount_id(inner.location()));
        if let Some(id) = write_mount_id {
            note_mount_write_open(id);
        }

        Self {
            inner,
            nonblock: AtomicBool::new(open_flags & O_NONBLOCK != 0),
            status_flags: AtomicU32::new(status_flags),
            write_mount_id,
        }
    }

    pub fn inner(&self) -> &axfs::File {
        &self.inner
    }

    pub fn is_path(&self) -> bool {
        self.inner.flags().contains(FileFlags::PATH)
    }

    fn is_blocking(&self) -> bool {
        self.inner.location().flags().contains(NodeFlags::BLOCKING)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if let Some(id) = self.write_mount_id {
            note_mount_write_close(id);
        }
    }
}

fn path_for(loc: &Location) -> Cow<'static, str> {
    loc.absolute_path()
        .map_or_else(|_| "<error>".into(), |f| Cow::Owned(f.to_string()))
}

impl FileLike for File {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        let inner = self.inner();
        let requested = dst.remaining_mut();
        let read = if likely(self.is_blocking()) {
            inner.read(dst)
        } else {
            block_on(poll_io(self, IoEvents::IN, self.nonblocking(), || {
                inner.read(&mut *dst)
            }))
        };
        if read.is_ok() && requested > 0 && self.status_flags() & O_NOATIME == 0 {
            record_file_access(inner);
        }
        read
    }

    fn write(&self, src: &mut IoSrc) -> AxResult<usize> {
        let inner = self.inner();
        let written = if likely(self.is_blocking()) {
            inner.write(src)
        } else {
            block_on(poll_io(self, IoEvents::OUT, self.nonblocking(), || {
                inner.write(&mut *src)
            }))
        }?;
        if written > 0 {
            let now = realtime_time();
            if !inner.record_modification(now) {
                let _ = update_cached_metadata(
                    inner.location(),
                    MetadataUpdate {
                        mtime: Some(now),
                        ctime: Some(now),
                        ..Default::default()
                    },
                );
            }
        }
        Ok(written)
    }

    fn stat(&self) -> AxResult<Kstat> {
        Ok(metadata_to_kstat(&self.inner().metadata()?))
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> AxResult<usize> {
        self.inner().backend()?.location().ioctl(cmd, arg)
    }

    fn set_nonblocking(&self, flag: bool) -> AxResult {
        self.nonblock.store(flag, Ordering::Release);
        if flag {
            self.status_flags.fetch_or(O_NONBLOCK, Ordering::AcqRel);
        } else {
            self.status_flags.fetch_and(!O_NONBLOCK, Ordering::AcqRel);
        }
        Ok(())
    }

    fn nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    fn status_flags(&self) -> u32 {
        self.status_flags.load(Ordering::Acquire)
    }

    fn set_status_flags(&self, flags: u32) -> AxResult {
        // Linux only allows changing a subset of status flags via F_SETFL.
        const MUTABLE_STATUS_BITS: u32 = O_APPEND | O_NONBLOCK;
        let old = self.status_flags.load(Ordering::Acquire);
        let new = (old & !MUTABLE_STATUS_BITS) | (flags & MUTABLE_STATUS_BITS);
        self.status_flags.store(new, Ordering::Release);
        self.nonblock
            .store(new & O_NONBLOCK != 0, Ordering::Release);
        Ok(())
    }

    fn path(&self) -> Cow<'_, str> {
        path_for(self.inner.location())
    }

    fn from_fd(fd: c_int) -> AxResult<Arc<Self>>
    where
        Self: Sized + 'static,
    {
        get_file_like(fd)?.downcast_arc().map_err(|any| {
            if any.is::<Directory>() {
                AxError::IsADirectory
            } else {
                AxError::BrokenPipe
            }
        })
    }
}
impl Pollable for File {
    fn poll(&self) -> IoEvents {
        self.inner().location().poll()
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        self.inner().location().register(context, events);
    }
}

/// Directory wrapper for `axfs::fops::Directory`.
pub struct Directory {
    inner: Location,
    pub offset: Mutex<u64>,
    status_flags: AtomicU32,
}

impl Directory {
    pub fn new(inner: Location) -> Self {
        Self::new_with_status_flags(inner, 0)
    }

    pub fn new_with_status_flags(inner: Location, status_flags: u32) -> Self {
        Self {
            inner,
            offset: Mutex::new(0),
            status_flags: AtomicU32::new(status_flags),
        }
    }

    /// Get the inner node of the directory.
    pub fn inner(&self) -> &Location {
        &self.inner
    }
}

impl FileLike for Directory {
    fn read(&self, _dst: &mut IoDst) -> AxResult<usize> {
        Err(AxError::IsADirectory)
    }

    fn write(&self, _src: &mut IoSrc) -> AxResult<usize> {
        Err(AxError::BadFileDescriptor)
    }

    fn stat(&self) -> AxResult<Kstat> {
        Ok(metadata_to_kstat(&self.inner.metadata()?))
    }

    fn path(&self) -> Cow<'_, str> {
        path_for(&self.inner)
    }

    fn status_flags(&self) -> u32 {
        self.status_flags.load(Ordering::Acquire)
    }

    fn set_status_flags(&self, flags: u32) -> AxResult {
        const MUTABLE_STATUS_BITS: u32 = O_NONBLOCK;
        let old = self.status_flags.load(Ordering::Acquire);
        let new = (old & !MUTABLE_STATUS_BITS) | (flags & MUTABLE_STATUS_BITS);
        self.status_flags.store(new, Ordering::Release);
        Ok(())
    }

    fn from_fd(fd: c_int) -> AxResult<Arc<Self>> {
        get_file_like(fd)?
            .downcast_arc()
            .map_err(|_| AxError::NotADirectory)
    }
}
impl Pollable for Directory {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}
