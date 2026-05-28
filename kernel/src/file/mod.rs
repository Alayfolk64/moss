mod bpf;
pub mod epoll;
pub mod event;
mod fs;
#[cfg(feature = "net")]
mod net;
mod pidfd;
mod pipe;
pub mod signalfd;
pub mod timerfd;

use alloc::{borrow::Cow, collections::BTreeMap, string::String, sync::Arc};
use core::{ffi::c_int, time::Duration};

use axerrno::{AxError, AxResult};
use axfs::{FS_CONTEXT, OpenOptions};
use axfs_ng_vfs::DeviceId;
use axio::prelude::*;
use axpoll::Pollable;
use axtask::current;
use downcast_rs::{DowncastSync, impl_downcast};
use flatten_objects::FlattenObjects;
use linux_raw_sys::general::{
    F_OWNER_PGRP, F_OWNER_PID, F_OWNER_TID, O_NONBLOCK, RLIMIT_NOFILE, S_IFBLK, S_IFMT,
    STATX_ATTR_APPEND, STATX_ATTR_COMPRESSED, STATX_ATTR_IMMUTABLE, STATX_ATTR_MOUNT_ROOT,
    STATX_ATTR_NODUMP, STATX_BASIC_STATS, STATX_BTIME, STATX_DIOALIGN, stat, statx,
    statx_timestamp,
};
use spin::RwLock;
use starry_signal::{SignalInfo, Signo};

#[cfg(feature = "net")]
pub use self::net::Socket;
pub use self::{
    bpf::{
        BPF_MAP_TYPE_ARRAY, BPF_MAP_TYPE_HASH, BPF_MAP_TYPE_RINGBUF, BpfMap, BpfProgram,
        BpfProgramKind,
    },
    fs::{
        Directory, File, inode_flags, register_special_node, resolve_at, set_inode_flags,
        update_atime_for_access, with_fs,
    },
    pidfd::PidFd,
    pipe::{PIPE_MAX_SIZE, Pipe},
    timerfd::TimerFd,
};
use crate::task::{
    AX_FILE_LIMIT, AsThread, send_signal_to_process, send_signal_to_process_group,
    send_signal_to_thread,
};

#[derive(Debug, Clone, Copy)]
pub struct Kstat {
    pub dev: u64,
    pub ino: u64,
    pub nlink: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blksize: u32,
    pub blocks: u64,
    pub rdev: DeviceId,
    pub attributes: u64,
    pub attributes_mask: u64,
    pub atime: Duration,
    pub btime: Duration,
    pub mtime: Duration,
    pub ctime: Duration,
}

impl Default for Kstat {
    fn default() -> Self {
        Self {
            dev: 0,
            ino: 1,
            nlink: 1,
            mode: 0,
            uid: 1,
            gid: 1,
            size: 0,
            blksize: 4096,
            blocks: 0,
            rdev: DeviceId::default(),
            attributes: 0,
            attributes_mask: 0,
            atime: Duration::default(),
            btime: Duration::default(),
            mtime: Duration::default(),
            ctime: Duration::default(),
        }
    }
}

impl From<Kstat> for stat {
    fn from(value: Kstat) -> Self {
        // SAFETY: valid for stat
        let mut stat: stat = unsafe { core::mem::zeroed() };
        stat.st_dev = value.dev as _;
        stat.st_ino = value.ino as _;
        stat.st_nlink = value.nlink as _;
        stat.st_mode = value.mode as _;
        stat.st_uid = value.uid as _;
        stat.st_gid = value.gid as _;
        stat.st_size = value.size as _;
        stat.st_blksize = value.blksize as _;
        stat.st_blocks = value.blocks as _;
        stat.st_rdev = value.rdev.0 as _;

        stat.st_atime = value.atime.as_secs() as _;
        stat.st_atime_nsec = value.atime.subsec_nanos() as _;
        stat.st_mtime = value.mtime.as_secs() as _;
        stat.st_mtime_nsec = value.mtime.subsec_nanos() as _;
        stat.st_ctime = value.ctime.as_secs() as _;
        stat.st_ctime_nsec = value.ctime.subsec_nanos() as _;

        stat
    }
}

impl From<Kstat> for statx {
    fn from(value: Kstat) -> Self {
        // SAFETY: valid for statx
        let mut statx: statx = unsafe { core::mem::zeroed() };
        statx.stx_mask = STATX_BASIC_STATS | STATX_BTIME;
        if value.mode & S_IFMT == S_IFBLK {
            statx.stx_mask |= STATX_DIOALIGN;
            statx.stx_dio_mem_align = 1;
            statx.stx_dio_offset_align = 512;
        }
        statx.stx_blksize = value.blksize as _;
        statx.stx_attributes = value.attributes;
        statx.stx_nlink = value.nlink as _;
        statx.stx_uid = value.uid as _;
        statx.stx_gid = value.gid as _;
        statx.stx_mode = value.mode as _;
        statx.stx_ino = value.ino as _;
        statx.stx_size = value.size as _;
        statx.stx_blocks = value.blocks as _;
        statx.stx_attributes_mask = value.attributes_mask
            | (STATX_ATTR_COMPRESSED
                | STATX_ATTR_IMMUTABLE
                | STATX_ATTR_APPEND
                | STATX_ATTR_NODUMP
                | STATX_ATTR_MOUNT_ROOT) as u64;
        statx.stx_rdev_major = value.rdev.major();
        statx.stx_rdev_minor = value.rdev.minor();

        fn time_to_statx(time: &Duration) -> statx_timestamp {
            statx_timestamp {
                tv_sec: time.as_secs() as _,
                tv_nsec: time.subsec_nanos() as _,
                __reserved: 0,
            }
        }
        statx.stx_atime = time_to_statx(&value.atime);
        statx.stx_btime = time_to_statx(&value.btime);
        statx.stx_ctime = time_to_statx(&value.ctime);
        statx.stx_mtime = time_to_statx(&value.mtime);

        statx.stx_dev_major = (value.dev >> 32) as _;
        statx.stx_dev_minor = value.dev as _;

        statx
    }
}

pub trait WriteBuf: Write + IoBufMut {}
impl<T: Write + IoBufMut> WriteBuf for T {}
pub type IoDst<'a> = dyn WriteBuf + 'a;

pub trait ReadBuf: Read + IoBuf {}
impl<T: Read + IoBuf> ReadBuf for T {}
pub type IoSrc<'a> = dyn ReadBuf + 'a;

#[allow(dead_code)]
pub trait FileLike: Pollable + DowncastSync {
    fn read(&self, _dst: &mut IoDst) -> AxResult<usize> {
        Err(AxError::InvalidInput)
    }

    fn write(&self, _src: &mut IoSrc) -> AxResult<usize> {
        Err(AxError::InvalidInput)
    }

    fn stat(&self) -> AxResult<Kstat> {
        Ok(Kstat::default())
    }

    fn path(&self) -> Cow<'_, str>;

    fn proc_fdinfo(&self) -> Option<String> {
        None
    }

    fn ioctl(&self, _cmd: u32, _arg: usize) -> AxResult<usize> {
        Err(AxError::NotATty)
    }

    fn nonblocking(&self) -> bool {
        false
    }

    fn set_nonblocking(&self, _nonblocking: bool) -> AxResult {
        Ok(())
    }

    fn status_flags(&self) -> u32 {
        if self.nonblocking() { O_NONBLOCK } else { 0 }
    }

    fn set_status_flags(&self, flags: u32) -> AxResult {
        self.set_nonblocking(flags & O_NONBLOCK != 0)
    }

    fn from_fd(fd: c_int) -> AxResult<Arc<Self>>
    where
        Self: Sized + 'static,
    {
        get_file_like(fd)?
            .downcast_arc()
            .map_err(|_| AxError::InvalidInput)
    }

    fn add_to_fd_table(self, cloexec: bool) -> AxResult<c_int>
    where
        Self: Sized + 'static,
    {
        add_file_like(Arc::new(self), cloexec)
    }
}
impl_downcast!(sync FileLike);

#[derive(Clone)]
pub struct FileDescriptor {
    pub inner: Arc<dyn FileLike>,
    pub cloexec: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct AsyncFcntlState {
    pub owner_type: i32,
    pub owner_pid: i32,
    pub signal: i32,
}

impl Default for AsyncFcntlState {
    fn default() -> Self {
        Self {
            owner_type: F_OWNER_PID as i32,
            owner_pid: 0,
            signal: 0,
        }
    }
}

static ASYNC_FCNTL_STATE: RwLock<BTreeMap<usize, AsyncFcntlState>> = RwLock::new(BTreeMap::new());

fn file_like_key(file: &Arc<dyn FileLike>) -> usize {
    if let Some(pipe) = file.downcast_ref::<Pipe>() {
        return pipe.async_key();
    }
    let ptr: *const dyn FileLike = Arc::as_ptr(file);
    ptr as *const () as usize
}

pub fn async_fcntl_set_owner(file: &Arc<dyn FileLike>, owner_type: i32, owner_pid: i32) {
    let mut map = ASYNC_FCNTL_STATE.write();
    let state = map.entry(file_like_key(file)).or_default();
    state.owner_type = owner_type;
    state.owner_pid = owner_pid;
}

pub fn async_fcntl_get_owner(file: &Arc<dyn FileLike>) -> AsyncFcntlState {
    ASYNC_FCNTL_STATE
        .read()
        .get(&file_like_key(file))
        .copied()
        .unwrap_or_default()
}

pub fn async_fcntl_set_signal(file: &Arc<dyn FileLike>, signal: i32) {
    ASYNC_FCNTL_STATE
        .write()
        .entry(file_like_key(file))
        .or_default()
        .signal = signal;
}

pub fn async_fcntl_get_signal(file: &Arc<dyn FileLike>) -> i32 {
    ASYNC_FCNTL_STATE
        .read()
        .get(&file_like_key(file))
        .map_or(0, |state| state.signal)
}

pub fn notify_async_io(file_data_ptr: usize) {
    let Some(state) = ASYNC_FCNTL_STATE.read().get(&file_data_ptr).copied() else {
        return;
    };
    if state.owner_pid <= 0 {
        return;
    }

    let signo = if state.signal == 0 {
        Signo::SIGIO
    } else if let Some(signo) = Signo::from_repr(state.signal as u8) {
        signo
    } else {
        return;
    };
    let sig = SignalInfo::new_kernel(signo);

    match state.owner_type as u32 {
        F_OWNER_TID => {
            let _ = send_signal_to_thread(None, state.owner_pid as _, Some(sig));
        }
        F_OWNER_PID => {
            let _ = send_signal_to_process(state.owner_pid as _, Some(sig));
        }
        F_OWNER_PGRP => {
            let _ = send_signal_to_process_group(state.owner_pid as _, Some(sig));
        }
        _ => {}
    }
}

scope_local::scope_local! {
    /// The current file descriptor table.
    pub static FD_TABLE: Arc<RwLock<FlattenObjects<FileDescriptor, AX_FILE_LIMIT>>> = Arc::default();
}

/// Get a file-like object by `fd`.
pub fn get_file_like(fd: c_int) -> AxResult<Arc<dyn FileLike>> {
    FD_TABLE
        .read()
        .get(fd as usize)
        .map(|fd| fd.inner.clone())
        .ok_or(AxError::BadFileDescriptor)
}

/// Add a file to the file descriptor table.
pub fn add_file_like(f: Arc<dyn FileLike>, cloexec: bool) -> AxResult<c_int> {
    let max_nofile = current().as_thread().proc_data.rlim.read()[RLIMIT_NOFILE].current;
    let mut table = FD_TABLE.write();
    if table.count() as u64 >= max_nofile {
        return Err(AxError::TooManyOpenFiles);
    }
    let fd = FileDescriptor { inner: f, cloexec };
    Ok(table.add(fd).map_err(|_| AxError::TooManyOpenFiles)? as c_int)
}

/// Add a file to the file descriptor table with a minimum fd hint.
pub fn add_file_like_from(f: Arc<dyn FileLike>, cloexec: bool, min_fd: usize) -> AxResult<c_int> {
    let max_nofile = current().as_thread().proc_data.rlim.read()[RLIMIT_NOFILE].current;
    let max_fd = max_nofile.min(AX_FILE_LIMIT as u64) as usize;
    if min_fd >= max_fd {
        return Err(AxError::InvalidInput);
    }

    let mut table = FD_TABLE.write();
    if table.count() as u64 >= max_nofile || table.count() >= max_fd {
        return Err(AxError::TooManyOpenFiles);
    }

    let new_fd = (min_fd..max_fd)
        .find(|fd| !table.is_assigned(*fd))
        .ok_or(AxError::TooManyOpenFiles)?;
    let fd = FileDescriptor { inner: f, cloexec };
    table
        .add_at(new_fd, fd)
        .map_err(|_| AxError::TooManyOpenFiles)?;
    Ok(new_fd as c_int)
}

/// Close a file by `fd`.
pub fn close_file_like(fd: c_int) -> AxResult {
    let f = FD_TABLE
        .write()
        .remove(fd as usize)
        .ok_or(AxError::BadFileDescriptor)?;
    debug!("close_file_like <= count: {}", Arc::strong_count(&f.inner));
    Ok(())
}

pub fn add_stdio(fd_table: &mut FlattenObjects<FileDescriptor, AX_FILE_LIMIT>) -> AxResult<()> {
    assert_eq!(fd_table.count(), 0);
    let cx = FS_CONTEXT.lock();
    let open = |options: &mut OpenOptions| {
        AxResult::Ok(Arc::new(File::new(
            options.open(&cx, "/dev/console")?.into_file()?,
        )))
    };

    let tty_in = open(OpenOptions::new().read(true).write(false))?;
    let tty_out = open(OpenOptions::new().read(false).write(true))?;
    fd_table
        .add(FileDescriptor {
            inner: tty_in,
            cloexec: false,
        })
        .map_err(|_| AxError::TooManyOpenFiles)?;
    fd_table
        .add(FileDescriptor {
            inner: tty_out.clone(),
            cloexec: false,
        })
        .map_err(|_| AxError::TooManyOpenFiles)?;
    fd_table
        .add(FileDescriptor {
            inner: tty_out,
            cloexec: false,
        })
        .map_err(|_| AxError::TooManyOpenFiles)?;

    Ok(())
}
