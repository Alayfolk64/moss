use alloc::{
    borrow::Cow,
    collections::VecDeque,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    ffi::{c_char, c_int},
    sync::atomic::{AtomicBool, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, OpenOptions, OpenResult};
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
    file::{Directory, File, FileLike, IoDst, add_file_like, close_file_like, resolve_at},
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
const FANOTIFY_DEFAULT_MAX_QUEUED_EVENTS: usize = 16_384;
const FAN_NOFD: c_int = -1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FanotifyTarget {
    dev: u64,
    ino: u64,
    is_dir: bool,
}

#[derive(Clone, Debug)]
struct FanotifyMark {
    target: FanotifyTarget,
    path: Option<String>,
    kind: FanotifyMarkKind,
    mask: u64,
    ignored_mask: u64,
    ignore_surv_modify: bool,
}

struct FanotifyEvent {
    mask: u64,
    file: Option<Arc<dyn FileLike>>,
    path: Option<String>,
    pid: i32,
}

struct FanotifyMarkTarget {
    target: FanotifyTarget,
    path: Option<String>,
    kind: FanotifyMarkKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FanotifyMarkKind {
    Inode,
    Mount,
    Filesystem,
}

static FANOTIFY_FDS: RwLock<Vec<Weak<FanotifyFd>>> = RwLock::new(Vec::new());

pub struct FanotifyFd {
    init_flags: u32,
    event_f_flags: u32,
    owner_pid: i32,
    unprivileged_listener: bool,
    nonblocking: AtomicBool,
    marks: Mutex<Vec<FanotifyMark>>,
    events: Mutex<VecDeque<FanotifyEvent>>,
    poll: PollSet,
}

impl FanotifyFd {
    fn new(init_flags: u32, event_f_flags: u32) -> Self {
        let curr = current();
        let proc_data = &curr.as_thread().proc_data;
        Self {
            init_flags,
            event_f_flags,
            owner_pid: proc_data.proc.pid() as i32,
            unprivileged_listener: proc_data.euid() != 0,
            nonblocking: AtomicBool::new(false),
            marks: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::new()),
            poll: PollSet::new(),
        }
    }

    fn reports_fid(&self) -> bool {
        self.init_flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME) != 0
    }

    fn reports_name(&self) -> bool {
        self.init_flags & FAN_REPORT_NAME != 0
    }

    fn reports_target_fid(&self) -> bool {
        self.init_flags & FAN_REPORT_TARGET_FID != 0
    }

    fn reports_tid(&self) -> bool {
        self.init_flags & FAN_REPORT_TID != 0
    }

    fn apply_mark(&self, flags: u32, mask: u64, target: Option<FanotifyMarkTarget>) {
        if flags & FAN_MARK_FLUSH != 0 {
            self.marks.lock().clear();
            self.events.lock().clear();
            return;
        }

        let Some(target) = target else {
            return;
        };
        let mut marks = self.marks.lock();
        let mark_is_ignored = flags & (FAN_MARK_IGNORED_MASK | FAN_MARK_IGNORE) != 0;
        if flags & FAN_MARK_REMOVE != 0 {
            if let Some(mark) = marks
                .iter_mut()
                .find(|mark| mark.target == target.target && mark.kind == target.kind)
            {
                if mark_is_ignored {
                    mark.ignored_mask &= !mask;
                    if flags & FAN_MARK_IGNORED_SURV_MODIFY != 0 {
                        mark.ignore_surv_modify = false;
                    }
                } else {
                    mark.mask &= !mask;
                }
            }
            marks.retain(|mark| mark.mask != 0 || mark.ignored_mask != 0);
        } else if let Some(mark) = marks
            .iter_mut()
            .find(|mark| mark.target == target.target && mark.kind == target.kind)
        {
            if mark_is_ignored {
                mark.ignored_mask |= mask;
                mark.ignore_surv_modify = flags & FAN_MARK_IGNORED_SURV_MODIFY != 0;
            } else {
                mark.mask |= mask;
            }
            mark.path = target.path;
        } else {
            marks.push(FanotifyMark {
                target: target.target,
                path: target.path,
                kind: target.kind,
                mask: if mark_is_ignored { 0 } else { mask },
                ignored_mask: if mark_is_ignored { mask } else { 0 },
                ignore_surv_modify: flags & FAN_MARK_IGNORED_SURV_MODIFY != 0,
            });
        }
    }

    fn queue_event(
        &self,
        target: FanotifyTarget,
        path: Option<&str>,
        file: Arc<dyn FileLike>,
        tgid: i32,
        tid: i32,
        mask: u64,
    ) {
        let mut matched_mask = 0;
        let mut ignored_mask = 0;
        {
            let mut marks = self.marks.lock();
            for mark in marks.iter_mut() {
                if !mark_matches_event(mark, target, path) {
                    continue;
                }

                ignored_mask |= mark.ignored_mask & mask;
                let event_on_dir_allowed = !target.is_dir || mark.mask & FAN_ONDIR != 0;
                if event_on_dir_allowed {
                    matched_mask |= mask & mark.mask;
                }
            }

            if mask & FAN_MODIFY != 0 {
                for mark in marks.iter_mut() {
                    if !mark.ignore_surv_modify && mark_matches_event(mark, target, path) {
                        mark.ignored_mask = 0;
                    }
                }
                marks.retain(|mark| mark.mask != 0 || mark.ignored_mask != 0);
            }
        }

        let event_mask = matched_mask & !ignored_mask;
        if event_mask == 0 {
            return;
        }

        let pid = if self.reports_tid() { tid } else { tgid };
        let pid = if self.unprivileged_listener && tgid != self.owner_pid {
            0
        } else {
            pid
        };
        let mut events = self.events.lock();
        if self.init_flags & FAN_UNLIMITED_QUEUE == 0
            && events.len() >= FANOTIFY_DEFAULT_MAX_QUEUED_EVENTS
        {
            queue_overflow_event(&mut events, pid);
            self.poll.wake();
            return;
        }

        events.push_back(FanotifyEvent {
            mask: event_mask,
            file: Some(file),
            path: path.map(ToString::to_string),
            pid,
        });
        self.poll.wake();
    }

    fn event_fd(&self, event: &FanotifyEvent) -> AxResult<c_int> {
        if self.reports_fid() {
            return Ok(FAN_NOFD);
        }

        let Some(event_file) = event.file.as_ref() else {
            return Ok(FAN_NOFD);
        };

        let Some(path) = event.path.as_deref() else {
            return add_file_like(event_file.clone(), self.event_f_flags & O_CLOEXEC != 0);
        };

        let file = open_fanotify_event_file(path, self.event_f_flags)
            .unwrap_or_else(|_| event_file.clone());
        add_file_like(file, self.event_f_flags & O_CLOEXEC != 0)
    }
}

fn queue_overflow_event(events: &mut VecDeque<FanotifyEvent>, pid: i32) {
    if events.iter().any(|event| event.mask == FAN_Q_OVERFLOW) {
        return;
    }

    while events.len() >= FANOTIFY_DEFAULT_MAX_QUEUED_EVENTS {
        events.pop_back();
    }
    events.push_back(FanotifyEvent {
        mask: FAN_Q_OVERFLOW,
        file: None,
        path: None,
        pid,
    });
}

fn mark_matches_event(mark: &FanotifyMark, target: FanotifyTarget, path: Option<&str>) -> bool {
    match mark.kind {
        FanotifyMarkKind::Inode => {
            let self_match = mark.target == target
                || mark
                    .path
                    .as_deref()
                    .zip(path)
                    .is_some_and(|(mark_path, event_path)| mark_path == event_path);
            self_match || mark_matches_child(mark, path)
        }
        FanotifyMarkKind::Mount | FanotifyMarkKind::Filesystem => mark
            .path
            .as_deref()
            .zip(path)
            .is_some_and(|(mark_path, event_path)| path_is_same_or_below(event_path, mark_path)),
    }
}

fn mark_matches_child(mark: &FanotifyMark, path: Option<&str>) -> bool {
    mark.mask & FAN_EVENT_ON_CHILD != 0
        && mark.target.is_dir
        && mark
            .path
            .as_deref()
            .zip(path)
            .and_then(|(mark_path, event_path)| {
                parent_path(event_path).map(|parent| (mark_path, parent))
            })
            .is_some_and(|(mark_path, parent)| mark_path == parent)
}

fn parent_path(path: &str) -> Option<&str> {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
}

fn path_is_same_or_below(path: &str, base: &str) -> bool {
    path == base
        || base == "/"
        || path
            .strip_prefix(base)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn open_fanotify_event_file(path: &str, event_f_flags: u32) -> AxResult<Arc<dyn FileLike>> {
    let mut options = OpenOptions::new();
    match event_f_flags & O_ACCMODE {
        linux_raw_sys::general::O_WRONLY => {
            options.write(true);
        }
        linux_raw_sys::general::O_RDWR => {
            options.read(true).write(true);
        }
        _ => {
            options.read(true);
        }
    };

    let file: Arc<dyn FileLike> = match options.open(&FS_CONTEXT.lock(), path)? {
        OpenResult::File(file) => Arc::new(File::new(file)),
        OpenResult::Dir(dir) => Arc::new(Directory::new(dir)),
    };
    if event_f_flags & O_NONBLOCK != 0 {
        file.set_nonblocking(true)?;
    }
    Ok(file)
}

impl FileLike for FanotifyFd {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        block_on(poll_io(self, IoEvents::IN, self.nonblocking(), || {
            let mut written = 0;

            loop {
                if dst.remaining_mut() < FANOTIFY_EVENT_METADATA_LEN {
                    return if written == 0 {
                        Err(AxError::InvalidInput)
                    } else {
                        Ok(written)
                    };
                }

                let Some(event) = self.events.lock().pop_front() else {
                    return if written == 0 {
                        Err(AxError::WouldBlock)
                    } else {
                        Ok(written)
                    };
                };

                let event_fd = self.event_fd(&event)?;
                let metadata = fanotify_event_metadata(event.mask, event_fd, event.pid);
                match dst.write(&metadata) {
                    Ok(0) => {
                        if event_fd >= 0 {
                            let _ = close_file_like(event_fd);
                        }
                        self.events.lock().push_front(event);
                        break;
                    }
                    Ok(n) => written += n,
                    Err(err) => {
                        if event_fd >= 0 {
                            let _ = close_file_like(event_fd);
                        }
                        self.events.lock().push_front(event);
                        return if written == 0 { Err(err) } else { Ok(written) };
                    }
                }
            }

            Ok(written)
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

fn notify_fanotify_event(file: Arc<dyn FileLike>, mask: u64) {
    let Ok(target) = target_from_file_like(&file) else {
        return;
    };
    let path = path_from_file_like(&file);
    let curr = current();
    let tgid = curr.as_thread().proc_data.proc.pid() as i32;
    let tid = curr.id().as_u64() as i32;
    let mut fds = FANOTIFY_FDS.write();
    fds.retain(|weak| {
        let Some(fd) = weak.upgrade() else {
            return false;
        };
        fd.queue_event(target, path.as_deref(), file.clone(), tgid, tid, mask);
        true
    });
}

pub(super) fn notify_fanotify_open(file: Arc<dyn FileLike>) {
    notify_fanotify_event(file, FAN_OPEN);
}

pub(crate) fn notify_fanotify_open_exec_path(path: &str) {
    if let Ok(file) = open_fanotify_event_file(path, 0) {
        notify_fanotify_event(file, FAN_OPEN | FAN_OPEN_EXEC);
    }
}

pub(super) fn notify_fanotify_access(file: Arc<dyn FileLike>) {
    notify_fanotify_event(file, FAN_ACCESS);
}

pub(super) fn notify_fanotify_modify(file: Arc<dyn FileLike>) {
    notify_fanotify_event(file, FAN_MODIFY);
}

pub(super) fn notify_fanotify_close(file: Arc<dyn FileLike>, writable: bool) {
    notify_fanotify_event(
        file,
        if writable {
            FAN_CLOSE_WRITE
        } else {
            FAN_CLOSE_NOWRITE
        },
    );
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

fn path_from_file_like(file: &Arc<dyn FileLike>) -> Option<String> {
    let path = file.path();
    if path.starts_with('<') {
        None
    } else {
        Some(path.to_string())
    }
}

fn target_from_mark(
    dirfd: c_int,
    path: Option<&str>,
    flags: u32,
) -> AxResult<Option<FanotifyMarkTarget>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let resolve_flags = if flags & FAN_MARK_DONT_FOLLOW != 0 {
        linux_raw_sys::general::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    let resolved = resolve_at(dirfd, Some(path), resolve_flags)?;
    let stat = resolved.stat()?;
    let loc = resolved.into_file();
    Ok(Some(FanotifyMarkTarget {
        target: target_from_stat(stat),
        kind: mark_kind_from_flags(flags),
        path: loc
            .and_then(|loc| loc.absolute_path().ok())
            .map(|path| path.to_string()),
    }))
}

fn mark_kind_from_flags(flags: u32) -> FanotifyMarkKind {
    if flags & FAN_MARK_MOUNT != 0 {
        FanotifyMarkKind::Mount
    } else if flags & FAN_MARK_FILESYSTEM != 0 {
        FanotifyMarkKind::Filesystem
    } else {
        FanotifyMarkKind::Inode
    }
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
