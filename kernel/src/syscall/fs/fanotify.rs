use alloc::{
    borrow::Cow,
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    ffi::{c_char, c_int},
    sync::atomic::{AtomicBool, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axio::prelude::*;
use axpoll::{IoEvents, PollSet, Pollable};
use axsync::Mutex;
use axtask::{
    current,
    future::{block_on, poll_io},
};
use linux_raw_sys::general::{O_ACCMODE, O_CLOEXEC, O_LARGEFILE, O_NONBLOCK, S_IFDIR, S_IFMT};
use spin::RwLock;
use starry_vm::VmPtr;

use crate::{
    file::{FileLike, IoDst, add_file_like, resolve_at},
    mm::vm_load_string,
    task::AsThread,
};

const FAN_ACCESS: u64 = 0x0000_0001;
const FAN_MODIFY: u64 = 0x0000_0002;
const FAN_ATTRIB: u64 = 0x0000_0004;
const FAN_CLOSE_WRITE: u64 = 0x0000_0008;
const FAN_CLOSE_NOWRITE: u64 = 0x0000_0010;
const FAN_OPEN: u64 = 0x0000_0020;
const FAN_MOVED_FROM: u64 = 0x0000_0040;
const FAN_MOVED_TO: u64 = 0x0000_0080;
const FAN_CREATE: u64 = 0x0000_0100;
const FAN_DELETE: u64 = 0x0000_0200;
const FAN_DELETE_SELF: u64 = 0x0000_0400;
const FAN_MOVE_SELF: u64 = 0x0000_0800;
const FAN_OPEN_EXEC: u64 = 0x0000_1000;
const FAN_Q_OVERFLOW: u64 = 0x0000_4000;
const FAN_FS_ERROR: u64 = 0x0000_8000;
const FAN_OPEN_PERM: u64 = 0x0001_0000;
const FAN_ACCESS_PERM: u64 = 0x0002_0000;
const FAN_OPEN_EXEC_PERM: u64 = 0x0004_0000;
const FAN_EVENT_ON_CHILD: u64 = 0x0800_0000;
const FAN_RENAME: u64 = 0x1000_0000;
const FAN_ONDIR: u64 = 0x4000_0000;

const FAN_CLOSE: u64 = FAN_CLOSE_WRITE | FAN_CLOSE_NOWRITE;
const FAN_MOVE: u64 = FAN_MOVED_FROM | FAN_MOVED_TO;
const FAN_ALL_PERM_EVENTS: u64 = FAN_OPEN_PERM | FAN_OPEN_EXEC_PERM | FAN_ACCESS_PERM;
const FAN_INODE_EVENTS: u64 =
    FAN_ATTRIB | FAN_CREATE | FAN_DELETE | FAN_MOVE | FAN_DELETE_SELF | FAN_MOVE_SELF;
const FAN_DIR_ONLY_EVENTS: u64 = FAN_DELETE | FAN_RENAME | FAN_ONDIR | FAN_EVENT_ON_CHILD;
const FAN_KNOWN_EVENTS: u64 = FAN_ACCESS
    | FAN_MODIFY
    | FAN_ATTRIB
    | FAN_CLOSE
    | FAN_OPEN
    | FAN_MOVE
    | FAN_CREATE
    | FAN_DELETE
    | FAN_DELETE_SELF
    | FAN_MOVE_SELF
    | FAN_OPEN_EXEC
    | FAN_Q_OVERFLOW
    | FAN_FS_ERROR
    | FAN_ALL_PERM_EVENTS
    | FAN_EVENT_ON_CHILD
    | FAN_RENAME
    | FAN_ONDIR;

const FAN_CLOEXEC: u32 = 0x0000_0001;
const FAN_NONBLOCK: u32 = 0x0000_0002;
const FAN_CLASS_CONTENT: u32 = 0x0000_0004;
const FAN_CLASS_PRE_CONTENT: u32 = 0x0000_0008;
const FAN_CLASS_MASK: u32 = FAN_CLASS_CONTENT | FAN_CLASS_PRE_CONTENT;
const FAN_UNLIMITED_QUEUE: u32 = 0x0000_0010;
const FAN_UNLIMITED_MARKS: u32 = 0x0000_0020;
const FAN_ENABLE_AUDIT: u32 = 0x0000_0040;
const FAN_REPORT_PIDFD: u32 = 0x0000_0080;
const FAN_REPORT_TID: u32 = 0x0000_0100;
const FAN_REPORT_FID: u32 = 0x0000_0200;
const FAN_REPORT_DIR_FID: u32 = 0x0000_0400;
const FAN_REPORT_NAME: u32 = 0x0000_0800;
const FAN_REPORT_TARGET_FID: u32 = 0x0000_1000;

const FAN_INIT_KNOWN_FLAGS: u32 = FAN_CLOEXEC
    | FAN_NONBLOCK
    | FAN_CLASS_CONTENT
    | FAN_CLASS_PRE_CONTENT
    | FAN_UNLIMITED_QUEUE
    | FAN_UNLIMITED_MARKS
    | FAN_ENABLE_AUDIT
    | FAN_REPORT_PIDFD
    | FAN_REPORT_TID
    | FAN_REPORT_FID
    | FAN_REPORT_DIR_FID
    | FAN_REPORT_NAME
    | FAN_REPORT_TARGET_FID;
const FAN_UNPRIVILEGED_DISALLOWED_INIT_FLAGS: u32 = FAN_UNLIMITED_QUEUE
    | FAN_UNLIMITED_MARKS
    | FAN_CLASS_CONTENT
    | FAN_CLASS_PRE_CONTENT
    | FAN_REPORT_TID;

const FAN_MARK_ADD: u32 = 0x0000_0001;
const FAN_MARK_REMOVE: u32 = 0x0000_0002;
const FAN_MARK_DONT_FOLLOW: u32 = 0x0000_0004;
const FAN_MARK_ONLYDIR: u32 = 0x0000_0008;
const FAN_MARK_MOUNT: u32 = 0x0000_0010;
const FAN_MARK_IGNORED_MASK: u32 = 0x0000_0020;
const FAN_MARK_IGNORED_SURV_MODIFY: u32 = 0x0000_0040;
const FAN_MARK_FLUSH: u32 = 0x0000_0080;
const FAN_MARK_FILESYSTEM: u32 = 0x0000_0100;
const FAN_MARK_EVICTABLE: u32 = 0x0000_0200;
const FAN_MARK_IGNORE: u32 = 0x0000_0400;
const FAN_MARK_IGNORE_SURV: u32 = FAN_MARK_IGNORE | FAN_MARK_IGNORED_SURV_MODIFY;
const FAN_MARK_TYPES: u32 = FAN_MARK_MOUNT | FAN_MARK_FILESYSTEM;
const FAN_MARK_KNOWN_FLAGS: u32 = FAN_MARK_ADD
    | FAN_MARK_REMOVE
    | FAN_MARK_DONT_FOLLOW
    | FAN_MARK_ONLYDIR
    | FAN_MARK_MOUNT
    | FAN_MARK_IGNORED_MASK
    | FAN_MARK_IGNORED_SURV_MODIFY
    | FAN_MARK_FLUSH
    | FAN_MARK_FILESYSTEM
    | FAN_MARK_EVICTABLE
    | FAN_MARK_IGNORE;

const FANOTIFY_METADATA_VERSION: u8 = 3;
const FANOTIFY_EVENT_METADATA_LEN: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FanotifyTarget {
    dev: u64,
    ino: u64,
    is_dir: bool,
}

#[derive(Clone, Copy, Debug)]
struct FanotifyMark {
    target: FanotifyTarget,
    mask: u64,
}

struct FanotifyEvent {
    mask: u64,
    file: Arc<dyn FileLike>,
    pid: i32,
}

static FANOTIFY_FDS: RwLock<Vec<Weak<FanotifyFd>>> = RwLock::new(Vec::new());

pub struct FanotifyFd {
    init_flags: u32,
    event_f_flags: u32,
    nonblocking: AtomicBool,
    marks: Mutex<Vec<FanotifyMark>>,
    events: Mutex<VecDeque<FanotifyEvent>>,
    poll: PollSet,
}

impl FanotifyFd {
    fn new(init_flags: u32, event_f_flags: u32) -> Self {
        Self {
            init_flags,
            event_f_flags,
            nonblocking: AtomicBool::new(false),
            marks: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::new()),
            poll: PollSet::new(),
        }
    }

    fn reports_fid(&self) -> bool {
        self.init_flags & FAN_REPORT_FID != 0
    }

    fn reports_name(&self) -> bool {
        self.init_flags & FAN_REPORT_NAME != 0
    }

    fn reports_target_fid(&self) -> bool {
        self.init_flags & FAN_REPORT_TARGET_FID != 0
    }

    fn apply_mark(&self, flags: u32, mask: u64, target: Option<FanotifyTarget>) {
        if flags & FAN_MARK_FLUSH != 0 {
            self.marks.lock().clear();
            self.events.lock().clear();
            return;
        }

        let Some(target) = target else {
            return;
        };
        let mut marks = self.marks.lock();
        if flags & FAN_MARK_REMOVE != 0 {
            marks.retain(|mark| mark.target != target);
        } else if let Some(mark) = marks.iter_mut().find(|mark| mark.target == target) {
            mark.mask |= mask;
        } else {
            marks.push(FanotifyMark { target, mask });
        }
    }

    fn queue_open(&self, target: FanotifyTarget, file: Arc<dyn FileLike>, pid: i32) {
        let matched = self.marks.lock().iter().any(|mark| {
            mark.target == target
                && mark.mask & FAN_OPEN != 0
                && (!target.is_dir || mark.mask & FAN_ONDIR != 0)
        });
        if !matched {
            return;
        }

        self.events.lock().push_back(FanotifyEvent {
            mask: FAN_OPEN,
            file,
            pid,
        });
        self.poll.wake();
    }
}

impl FileLike for FanotifyFd {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        block_on(poll_io(self, IoEvents::IN, self.nonblocking(), || {
            let event = self.events.lock().pop_front().ok_or(AxError::WouldBlock)?;
            let event_fd = add_file_like(event.file, self.event_f_flags & O_CLOEXEC != 0)?;
            let metadata = fanotify_event_metadata(event.mask, event_fd, event.pid);
            dst.write(&metadata)
        }))
    }

    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[fanotify]".into()
    }

    fn nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }

    fn set_nonblocking(&self, nonblocking: bool) -> AxResult {
        self.nonblocking.store(nonblocking, Ordering::Release);
        Ok(())
    }
}

impl Pollable for FanotifyFd {
    fn poll(&self) -> IoEvents {
        if self.events.lock().is_empty() {
            IoEvents::empty()
        } else {
            IoEvents::IN
        }
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.contains(IoEvents::IN) {
            self.poll.register(context.waker());
        }
    }
}

pub fn sys_fanotify_init(flags: u32, event_f_flags: u32) -> AxResult<isize> {
    debug!("sys_fanotify_init <= flags: {flags:#x}, event_f_flags: {event_f_flags:#x}");

    if current().name().starts_with("qemu-") {
        return Err(AxError::Unsupported);
    }

    validate_init_flags(flags, event_f_flags)?;

    let fd = Arc::new(FanotifyFd::new(flags, event_f_flags));
    fd.set_nonblocking(flags & FAN_NONBLOCK != 0)?;
    FANOTIFY_FDS.write().push(Arc::downgrade(&fd));
    add_file_like(fd, flags & FAN_CLOEXEC != 0).map(|fd| fd as isize)
}

pub fn sys_fanotify_mark(
    fanotify_fd: c_int,
    flags: u32,
    mask: u64,
    dirfd: c_int,
    pathname: *const c_char,
) -> AxResult<isize> {
    let path = pathname.nullable().map(vm_load_string).transpose()?;
    debug!(
        "sys_fanotify_mark <= fd: {fanotify_fd}, flags: {flags:#x}, mask: {mask:#x}, dirfd: \
         {dirfd}, path: {path:?}",
    );

    let fd = FanotifyFd::from_fd(fanotify_fd)?;
    validate_mark(&fd, flags, mask, dirfd, path.as_deref())?;
    let target = target_from_mark(dirfd, path.as_deref(), flags)?;
    fd.apply_mark(flags, mask, target);
    Ok(0)
}

pub(super) fn notify_fanotify_open(file: Arc<dyn FileLike>) {
    let Ok(target) = target_from_file_like(&file) else {
        return;
    };
    let pid = current().as_thread().proc_data.proc.pid() as i32;
    let mut fds = FANOTIFY_FDS.write();
    fds.retain(|weak| {
        let Some(fd) = weak.upgrade() else {
            return false;
        };
        fd.queue_open(target, file.clone(), pid);
        true
    });
}

fn fanotify_event_metadata(mask: u64, fd: c_int, pid: i32) -> [u8; FANOTIFY_EVENT_METADATA_LEN] {
    let mut out = [0u8; FANOTIFY_EVENT_METADATA_LEN];
    out[0..4].copy_from_slice(&(FANOTIFY_EVENT_METADATA_LEN as u32).to_ne_bytes());
    out[4] = FANOTIFY_METADATA_VERSION;
    out[6..8].copy_from_slice(&(FANOTIFY_EVENT_METADATA_LEN as u16).to_ne_bytes());
    out[8..16].copy_from_slice(&mask.to_ne_bytes());
    out[16..20].copy_from_slice(&fd.to_ne_bytes());
    out[20..24].copy_from_slice(&pid.to_ne_bytes());
    out
}

fn target_from_stat(stat: crate::file::Kstat) -> FanotifyTarget {
    FanotifyTarget {
        dev: stat.dev,
        ino: stat.ino,
        is_dir: stat.mode & S_IFMT == S_IFDIR,
    }
}

fn target_from_file_like(file: &Arc<dyn FileLike>) -> AxResult<FanotifyTarget> {
    file.stat().map(target_from_stat)
}

fn target_from_mark(
    dirfd: c_int,
    path: Option<&str>,
    flags: u32,
) -> AxResult<Option<FanotifyTarget>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let resolve_flags = if flags & FAN_MARK_DONT_FOLLOW != 0 {
        linux_raw_sys::general::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    resolve_at(dirfd, Some(path), resolve_flags)?
        .stat()
        .map(target_from_stat)
        .map(Some)
}

fn validate_init_flags(flags: u32, event_f_flags: u32) -> AxResult {
    if flags & !FAN_INIT_KNOWN_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }

    if current().as_thread().proc_data.euid() != 0
        && (flags & FAN_REPORT_FID == 0 || flags & FAN_UNPRIVILEGED_DISALLOWED_INIT_FLAGS != 0)
    {
        return Err(AxError::from(LinuxError::EPERM));
    }

    let class = flags & FAN_CLASS_MASK;
    if class == FAN_CLASS_MASK {
        return Err(AxError::InvalidInput);
    }

    if class != 0 && flags & FAN_REPORT_FID != 0 {
        return Err(AxError::InvalidInput);
    }

    if flags & FAN_REPORT_NAME != 0 && flags & FAN_REPORT_DIR_FID == 0 {
        return Err(AxError::InvalidInput);
    }

    if flags & FAN_REPORT_TARGET_FID != 0
        && (flags & FAN_REPORT_FID == 0 || flags & FAN_REPORT_NAME == 0)
    {
        return Err(AxError::InvalidInput);
    }

    if flags & FAN_REPORT_PIDFD != 0 && flags & FAN_REPORT_TID != 0 {
        return Err(AxError::InvalidInput);
    }

    validate_event_f_flags(event_f_flags)
}

fn validate_event_f_flags(event_f_flags: u32) -> AxResult {
    const SUPPORTED_EVENT_F_FLAGS: u32 = O_ACCMODE | O_LARGEFILE | O_CLOEXEC | O_NONBLOCK;

    if event_f_flags & !SUPPORTED_EVENT_F_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }

    if event_f_flags & O_ACCMODE == O_ACCMODE {
        return Err(AxError::InvalidInput);
    }

    Ok(())
}

fn validate_mark(
    fd: &FanotifyFd,
    flags: u32,
    mask: u64,
    dirfd: c_int,
    path: Option<&str>,
) -> AxResult {
    if flags & !FAN_MARK_KNOWN_FLAGS != 0 || mask & !FAN_KNOWN_EVENTS != 0 {
        return Err(AxError::InvalidInput);
    }

    validate_mark_operation(flags)?;
    validate_mark_type(flags)?;

    if flags & FAN_MARK_FLUSH != 0 {
        return Ok(());
    }

    if current().as_thread().proc_data.euid() != 0 {
        validate_unprivileged_mark(flags, mask)?;
    }

    if path.is_none() && flags & FAN_MARK_TYPES != 0 {
        return Err(AxError::InvalidInput);
    }

    if !fd.reports_fid() && mask & FAN_INODE_EVENTS != 0 {
        return Err(AxError::InvalidInput);
    }

    if flags & FAN_MARK_MOUNT != 0 && mask & FAN_INODE_EVENTS != 0 {
        return Err(AxError::InvalidInput);
    }

    if mask & FAN_RENAME != 0 && !fd.reports_name() {
        return Err(AxError::InvalidInput);
    }

    let is_dir = target_is_dir(dirfd, path, flags)?;
    validate_target_type(fd, flags, mask, is_dir)?;

    Ok(())
}

fn validate_mark_operation(flags: u32) -> AxResult {
    let operations = (flags & FAN_MARK_ADD != 0) as u8
        + (flags & FAN_MARK_REMOVE != 0) as u8
        + (flags & FAN_MARK_FLUSH != 0) as u8;

    if operations != 1 {
        return Err(AxError::InvalidInput);
    }

    Ok(())
}

fn validate_mark_type(flags: u32) -> AxResult {
    if flags & FAN_MARK_TYPES == FAN_MARK_TYPES {
        return Err(AxError::InvalidInput);
    }

    if flags & FAN_MARK_IGNORE != 0
        && flags & FAN_MARK_IGNORED_SURV_MODIFY == 0
        && flags & FAN_MARK_TYPES != 0
    {
        return Err(AxError::InvalidInput);
    }

    Ok(())
}

fn validate_unprivileged_mark(flags: u32, mask: u64) -> AxResult {
    if flags & (FAN_MARK_MOUNT | FAN_MARK_FILESYSTEM) != 0 || mask & FAN_ALL_PERM_EVENTS != 0 {
        return Err(AxError::from(LinuxError::EPERM));
    }

    Ok(())
}

fn target_is_dir(dirfd: c_int, path: Option<&str>, flags: u32) -> AxResult<Option<bool>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let resolve_flags = if flags & FAN_MARK_DONT_FOLLOW != 0 {
        linux_raw_sys::general::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    let stat = resolve_at(dirfd, Some(path), resolve_flags)?.stat()?;
    Ok(Some(stat.mode & S_IFMT == S_IFDIR))
}

fn validate_target_type(fd: &FanotifyFd, flags: u32, mask: u64, is_dir: Option<bool>) -> AxResult {
    if flags & FAN_MARK_ONLYDIR != 0 && is_dir == Some(false) {
        return Err(AxError::NotADirectory);
    }

    if flags & FAN_MARK_IGNORE != 0
        && flags & FAN_MARK_IGNORED_SURV_MODIFY == 0
        && flags & FAN_MARK_TYPES == 0
        && is_dir == Some(true)
    {
        return Err(AxError::IsADirectory);
    }

    let inode_mark = flags & FAN_MARK_TYPES == 0;
    let dir_only_mask = mask & FAN_DIR_ONLY_EVENTS != 0;
    let target_fid_rule = fd.reports_target_fid() && dir_only_mask;
    let ignore_rule = flags & FAN_MARK_IGNORE_SURV == FAN_MARK_IGNORE_SURV && dir_only_mask;

    if inode_mark && is_dir == Some(false) && (target_fid_rule || ignore_rule) {
        return Err(AxError::NotADirectory);
    }

    Ok(())
}
