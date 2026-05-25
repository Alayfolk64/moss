use alloc::{
    borrow::Cow,
    collections::VecDeque,
    format,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    ffi::{c_char, c_int},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, OpenOptions, OpenResult};
use axfs_ng_vfs::path::Path;
use axio::prelude::*;
use axpoll::{IoEvents, PollSet, Pollable};
use axsync::Mutex;
use axtask::{
    current,
    future::{block_on, poll_io},
};
use event_listener::{Event, listener};
use linux_raw_sys::general::{O_ACCMODE, O_CLOEXEC, O_LARGEFILE, O_NONBLOCK, S_IFDIR, S_IFMT};
use spin::RwLock;
use starry_vm::VmPtr;

use crate::{
    file::{
        Directory, File, FileLike, IoDst, IoSrc, PidFd, add_file_like, close_file_like, resolve_at,
        with_fs,
    },
    mm::vm_load_string,
    task::{AsThread, get_process_data},
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
pub(crate) const FANOTIFY_MAX_QUEUED_EVENTS: usize = 128;
pub(crate) const FANOTIFY_MAX_USER_GROUPS: usize = 129;
pub(crate) const FANOTIFY_MAX_USER_MARKS: usize = 8192;
const FANOTIFY_EVENT_INFO_TYPE_FID: u8 = 1;
const FANOTIFY_EVENT_INFO_TYPE_DFID_NAME: u8 = 2;
const FANOTIFY_EVENT_INFO_TYPE_DFID: u8 = 3;
const FANOTIFY_EVENT_INFO_TYPE_PIDFD: u8 = 4;
const FANOTIFY_EVENT_INFO_TYPE_OLD_DFID_NAME: u8 = 10;
const FANOTIFY_EVENT_INFO_TYPE_NEW_DFID_NAME: u8 = 12;
const FANOTIFY_EVENT_INFO_FID_HEADER_LEN: usize = 12;
const FANOTIFY_FILE_HANDLE_HEADER_LEN: usize = 8;
const FANOTIFY_EVENT_INFO_PIDFD_LEN: usize = 8;
const FANOTIFY_FILE_HANDLE_BYTES: usize = 16;
const FANOTIFY_FILE_HANDLE_TYPE: i32 = 1;
const FAN_NOFD: c_int = -1;
const FAN_NOPIDFD: c_int = FAN_NOFD;
const FAN_ALLOW: u32 = 0x01;
const FAN_DENY: u32 = 0x02;
const FANOTIFY_RESPONSE_LEN: usize = 8;

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
    evictable: bool,
}

struct FanotifyEvent {
    mask: u64,
    file: Option<Arc<dyn FileLike>>,
    path: Option<String>,
    name: Option<String>,
    fid: Option<FanotifyTarget>,
    fid_info_type: u8,
    child_fid: Option<FanotifyTarget>,
    child_info_type: u8,
    child_name: Option<String>,
    pid: i32,
    permission_id: Option<u64>,
}

struct FanotifyPermission {
    id: u64,
    fd: Option<c_int>,
    response: Option<u32>,
}

struct FanotifyPathFile {
    path: String,
}

impl FileLike for FanotifyPathFile {
    fn path(&self) -> Cow<'_, str> {
        self.path.as_str().into()
    }
}

impl Pollable for FanotifyPathFile {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

pub(super) struct FanotifyPreparedEvent {
    target: FanotifyTarget,
    child_target: Option<FanotifyTarget>,
    child_info_type: u8,
    child_name: Option<String>,
    child_path: Option<String>,
    object_target: Option<FanotifyTarget>,
    path: String,
    file: Arc<dyn FileLike>,
    mask: u64,
    event_is_dir: bool,
}

pub(super) struct FanotifyPathAlias {
    target: FanotifyTarget,
    path: String,
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
static FANOTIFY_PATH_ALIASES: RwLock<Vec<FanotifyPathAlias>> = RwLock::new(Vec::new());

pub struct FanotifyFd {
    init_flags: u32,
    event_f_flags: u32,
    owner_pid: i32,
    unprivileged_listener: bool,
    nonblocking: AtomicBool,
    destroyed: AtomicBool,
    marks: Mutex<Vec<FanotifyMark>>,
    events: Mutex<VecDeque<FanotifyEvent>>,
    permissions: Mutex<Vec<FanotifyPermission>>,
    next_permission_id: AtomicU64,
    permission_event: Event,
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
            destroyed: AtomicBool::new(false),
            marks: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::new()),
            permissions: Mutex::new(Vec::new()),
            next_permission_id: AtomicU64::new(1),
            permission_event: Event::new(),
            poll: PollSet::new(),
        }
    }

    fn reports_fid(&self) -> bool {
        self.init_flags & (FAN_REPORT_FID | FAN_REPORT_DIR_FID | FAN_REPORT_NAME) != 0
    }

    fn reports_name(&self) -> bool {
        self.init_flags & FAN_REPORT_NAME != 0
    }

    fn reports_dir_fid(&self) -> bool {
        self.init_flags & (FAN_REPORT_DIR_FID | FAN_REPORT_NAME) != 0
    }

    fn reports_target_fid(&self) -> bool {
        self.init_flags & FAN_REPORT_TARGET_FID != 0
    }

    fn reports_pidfd(&self) -> bool {
        self.init_flags & FAN_REPORT_PIDFD != 0
    }

    fn reports_tid(&self) -> bool {
        self.init_flags & FAN_REPORT_TID != 0
    }

    fn is_destroyed(&self) -> bool {
        self.destroyed.load(Ordering::Acquire)
    }

    fn destroy(&self) {
        if self.destroyed.swap(true, Ordering::AcqRel) {
            return;
        }

        self.events.lock().clear();
        for permission in self.permissions.lock().iter_mut() {
            if permission.response.is_none() {
                permission.response = Some(FAN_ALLOW);
            }
        }
        self.permission_event.notify(usize::MAX);
        self.poll.wake();
    }

    fn destroy_on_owner_close(&self) {
        let curr = current();
        if curr.as_thread().proc_data.proc.pid() as i32 == self.owner_pid {
            self.destroy();
        }
    }

    fn apply_mark(&self, flags: u32, mask: u64, target: Option<FanotifyMarkTarget>) -> AxResult {
        if flags & FAN_MARK_FLUSH != 0 {
            self.marks.lock().clear();
            self.events.lock().clear();
            return Ok(());
        }

        let Some(target) = target else {
            return Ok(());
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
            } else {
                return Err(AxError::NotFound);
            }
            marks.retain(|mark| mark.mask != 0 || mark.ignored_mask != 0);
        } else if let Some(mark) = marks
            .iter_mut()
            .find(|mark| mark.target == target.target && mark.kind == target.kind)
        {
            let evictable = flags & FAN_MARK_EVICTABLE != 0;
            if evictable && !mark.evictable {
                return Err(AxError::AlreadyExists);
            }
            if !evictable {
                mark.evictable = false;
            }
            if mark_is_ignored {
                mark.ignored_mask |= mask;
                mark.ignore_surv_modify = flags & FAN_MARK_IGNORED_SURV_MODIFY != 0;
            } else {
                mark.mask |= mask;
            }
            mark.path = target.path;
        } else {
            if self.init_flags & FAN_UNLIMITED_MARKS == 0 && marks.len() >= FANOTIFY_MAX_USER_MARKS
            {
                return Err(AxError::StorageFull);
            }
            marks.push(FanotifyMark {
                target: target.target,
                path: target.path,
                kind: target.kind,
                mask: if mark_is_ignored { 0 } else { mask },
                ignored_mask: if mark_is_ignored { mask } else { 0 },
                ignore_surv_modify: flags & FAN_MARK_IGNORED_SURV_MODIFY != 0,
                evictable: flags & FAN_MARK_EVICTABLE != 0,
            });
        }
        Ok(())
    }

    fn queue_event(
        &self,
        target: FanotifyTarget,
        path: Option<&str>,
        file: Arc<dyn FileLike>,
        child_target: Option<FanotifyTarget>,
        child_info_type: u8,
        child_name: Option<&str>,
        child_path: Option<&str>,
        object_target: Option<FanotifyTarget>,
        tgid: i32,
        tid: i32,
        mask: u64,
        event_is_dir: bool,
        permission_id: Option<u64>,
    ) -> bool {
        if self.is_destroyed() {
            return false;
        }

        let mut matched_mask = 0;
        let mut ignored_mask = 0;
        let mut event_name = None;
        let mut child_matched_mask = 0;
        let mut child_ignored_mask = 0;
        let mut child_event_name = None;
        {
            let mut marks = self.marks.lock();
            for mark in marks.iter_mut() {
                if mark_matches_event_mask(
                    mark,
                    target,
                    path,
                    mark.ignored_mask,
                    mask,
                    event_is_dir,
                ) {
                    ignored_mask |= mark.ignored_mask & mask;
                    if self.reports_fid() && event_is_dir && mark.ignored_mask & FAN_ONDIR != 0 {
                        ignored_mask |= FAN_ONDIR;
                    }
                }
                if mask & FAN_RENAME != 0 && mark_ignores_rename(mark, target, path) {
                    ignored_mask |= FAN_RENAME;
                    if event_is_dir {
                        ignored_mask |= FAN_ONDIR;
                    }
                }
                if mask & FAN_MOVE != 0 && mark_ignores_child_move(mark, target, path) {
                    ignored_mask |= mask & FAN_MOVE;
                    if event_is_dir {
                        ignored_mask |= FAN_ONDIR;
                    }
                }

                if mark_matches_event_mask(mark, target, path, mark.mask, mask, event_is_dir) {
                    matched_mask |= mask & mark.mask;
                    if self.reports_fid() && event_is_dir && mark.mask & FAN_ONDIR != 0 {
                        matched_mask |= FAN_ONDIR;
                    }
                    if self.reports_name() {
                        if let Some(candidate) = fanotify_event_name(mark, path) {
                            if event_name
                                .as_ref()
                                .map_or(true, |(_, priority)| candidate.1 > *priority)
                            {
                                event_name = Some(candidate);
                            }
                        }
                    }
                }

                if mask & FAN_RENAME != 0 {
                    if let Some(child_target) = child_target {
                        if mark_matches_event_mask(
                            mark,
                            child_target,
                            child_path,
                            mark.ignored_mask,
                            mask,
                            event_is_dir,
                        ) {
                            child_ignored_mask |= mark.ignored_mask & mask;
                            if self.reports_fid()
                                && event_is_dir
                                && mark.ignored_mask & FAN_ONDIR != 0
                            {
                                child_ignored_mask |= FAN_ONDIR;
                            }
                        }
                        if mark_ignores_rename(mark, child_target, child_path) {
                            child_ignored_mask |= FAN_RENAME;
                            if event_is_dir {
                                child_ignored_mask |= FAN_ONDIR;
                            }
                        }
                    }
                }

                if mask & FAN_RENAME != 0 {
                    if let Some(child_target) = child_target {
                        if mark_matches_event_mask(
                            mark,
                            child_target,
                            child_path,
                            mark.mask,
                            mask,
                            event_is_dir,
                        ) {
                            child_matched_mask |= mask & mark.mask;
                            if self.reports_fid() && event_is_dir && mark.mask & FAN_ONDIR != 0 {
                                child_matched_mask |= FAN_ONDIR;
                            }
                            if self.reports_name() {
                                if let Some(candidate) = fanotify_event_name(mark, child_path) {
                                    if child_event_name
                                        .as_ref()
                                        .map_or(true, |(_, priority)| candidate.1 > *priority)
                                    {
                                        child_event_name = Some(candidate);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if mask & FAN_MODIFY != 0 {
                for mark in marks.iter_mut() {
                    if !mark.ignore_surv_modify
                        && mark_matches_event_mask(
                            mark,
                            target,
                            path,
                            mark.ignored_mask,
                            mask,
                            event_is_dir,
                        )
                    {
                        mark.ignored_mask = 0;
                    }
                }
                marks.retain(|mark| mark.mask != 0 || mark.ignored_mask != 0);
            }
        }

        let mut event_mask = if mask & FAN_RENAME != 0 {
            (matched_mask | child_matched_mask) & !(ignored_mask | child_ignored_mask)
        } else {
            matched_mask & !ignored_mask
        };
        if !event_is_dir && self.init_flags & FAN_REPORT_FID == 0 {
            event_mask &= !(FAN_DELETE_SELF | FAN_MOVE_SELF);
        }
        if event_mask == 0 {
            return false;
        }

        let pid = if self.reports_tid() { tid } else { tgid };
        let pid = if self.unprivileged_listener && tgid != self.owner_pid {
            0
        } else {
            pid
        };

        let self_event = event_mask & (FAN_DELETE_SELF | FAN_MOVE_SELF) != 0;
        let rename_event = event_mask & FAN_RENAME != 0;
        let old_rename_matched = rename_event && matched_mask & event_mask != 0;
        let new_rename_matched = rename_event && child_matched_mask & event_mask != 0;
        let dirent_event = child_target.is_some() && (!rename_event || old_rename_matched);
        if self_event && !event_is_dir {
            event_name = None;
        }
        if self.reports_name() && event_is_dir && !dirent_event {
            event_name = Some((".".to_string(), 3));
        }

        let mut report_fid = self.reports_fid().then_some(target);
        let mut report_info_type = FANOTIFY_EVENT_INFO_TYPE_FID;
        let mut report_child_fid = None;
        let mut report_child_info_type = child_info_type;
        if self.reports_fid() {
            if self.reports_dir_fid() {
                if new_rename_matched && !old_rename_matched {
                    if let Some(child_target) = child_target {
                        report_fid = Some(child_target);
                        event_name = child_event_name;
                        report_info_type = if event_name.is_some() {
                            FANOTIFY_EVENT_INFO_TYPE_NEW_DFID_NAME
                        } else {
                            FANOTIFY_EVENT_INFO_TYPE_DFID
                        };
                        if self.reports_target_fid() {
                            report_child_fid = object_target;
                            report_child_info_type = FANOTIFY_EVENT_INFO_TYPE_FID;
                        }
                    }
                } else if dirent_event || event_is_dir {
                    report_info_type = if rename_event && event_name.is_some() {
                        FANOTIFY_EVENT_INFO_TYPE_OLD_DFID_NAME
                    } else if event_name.is_some() {
                        FANOTIFY_EVENT_INFO_TYPE_DFID_NAME
                    } else {
                        FANOTIFY_EVENT_INFO_TYPE_DFID
                    };
                    if rename_event && old_rename_matched && !new_rename_matched {
                        if self.reports_target_fid() {
                            report_child_fid = object_target;
                            report_child_info_type = FANOTIFY_EVENT_INFO_TYPE_FID;
                        }
                    } else if !rename_event || new_rename_matched {
                        report_child_fid = child_target.filter(|_| {
                            child_info_type != FANOTIFY_EVENT_INFO_TYPE_FID
                                || self.reports_target_fid()
                        });
                    }
                } else if !self_event
                    && let Some(parent) = path.and_then(fanotify_parent_target_from_path)
                {
                    report_fid = Some(parent);
                    report_info_type = if event_name.is_some() {
                        FANOTIFY_EVENT_INFO_TYPE_DFID_NAME
                    } else {
                        FANOTIFY_EVENT_INFO_TYPE_DFID
                    };
                    if self.init_flags & FAN_REPORT_FID != 0 {
                        report_child_fid = Some(target);
                    }
                }
            }
            if !self.reports_dir_fid() && event_name.is_some() {
                report_info_type = FANOTIFY_EVENT_INFO_TYPE_DFID_NAME;
            }
        }

        let report_name = event_name.map(|(name, _)| name);
        let report_child_name = if report_child_fid.is_some()
            && report_child_info_type != FANOTIFY_EVENT_INFO_TYPE_FID
        {
            child_name.map(ToString::to_string)
        } else {
            None
        };
        if report_child_fid.is_none() {
            report_child_info_type = FANOTIFY_EVENT_INFO_TYPE_FID;
        }
        let mut events = self.events.lock();
        if permission_id.is_none() && self.reports_fid() {
            if let Some(event) = events.iter_mut().find(|event| {
                event.fid == report_fid
                    && event.fid_info_type == report_info_type
                    && event.child_fid == report_child_fid
                    && event.child_info_type == report_child_info_type
                    && event.child_name.as_ref() == report_child_name.as_ref()
                    && event.name.as_ref() == report_name.as_ref()
                    && event.pid == pid
                    && fanotify_can_merge_masks(
                        event.mask,
                        event_mask,
                        self.reports_name(),
                        self.reports_target_fid(),
                    )
            }) {
                event.mask |= event_mask;
                self.poll.wake();
                return true;
            }
        }

        if self.init_flags & FAN_UNLIMITED_QUEUE == 0 && events.len() >= FANOTIFY_MAX_QUEUED_EVENTS
        {
            if permission_id.is_none() {
                queue_overflow_event(&mut events, pid);
                self.poll.wake();
            }
            return false;
        }

        let event = FanotifyEvent {
            mask: event_mask,
            file: Some(file),
            path: path.map(ToString::to_string),
            name: report_name,
            fid: report_fid,
            fid_info_type: report_info_type,
            child_fid: report_child_fid,
            child_info_type: report_child_info_type,
            child_name: report_child_name,
            pid,
            permission_id,
        };
        if let Some(pos) = fanotify_insert_before_pending_close(&events, &event) {
            events.insert(pos, event);
        } else {
            events.push_back(event);
        }
        self.poll.wake();
        true
    }

    fn queue_permission_event(
        &self,
        target: FanotifyTarget,
        path: Option<&str>,
        file: Arc<dyn FileLike>,
        child_target: Option<FanotifyTarget>,
        child_info_type: u8,
        child_name: Option<&str>,
        child_path: Option<&str>,
        object_target: Option<FanotifyTarget>,
        tgid: i32,
        tid: i32,
        mask: u64,
        event_is_dir: bool,
    ) -> Option<u64> {
        if self.is_destroyed() {
            return None;
        }

        let id = self.next_permission_id.fetch_add(1, Ordering::AcqRel);
        self.permissions.lock().push(FanotifyPermission {
            id,
            fd: None,
            response: None,
        });
        if self.queue_event(
            target,
            path,
            file,
            child_target,
            child_info_type,
            child_name,
            child_path,
            object_target,
            tgid,
            tid,
            mask,
            event_is_dir,
            Some(id),
        ) {
            Some(id)
        } else {
            self.remove_permission(id);
            None
        }
    }

    fn set_permission_fd(&self, id: u64, fd: c_int) {
        if fd < 0 {
            return;
        }
        if let Some(permission) = self
            .permissions
            .lock()
            .iter_mut()
            .find(|permission| permission.id == id)
        {
            permission.fd = Some(fd);
        }
    }

    fn set_permission_response(&self, fd: c_int, response: u32) -> AxResult {
        let mut permissions = self.permissions.lock();
        let Some(permission) = permissions
            .iter_mut()
            .find(|permission| permission.fd == Some(fd) && permission.response.is_none())
        else {
            return Err(AxError::InvalidInput);
        };
        permission.response = Some(response);
        drop(permissions);
        self.permission_event.notify(usize::MAX);
        Ok(())
    }

    fn permission_response(&self, id: u64) -> Option<u32> {
        self.permissions
            .lock()
            .iter()
            .find(|permission| permission.id == id)
            .and_then(|permission| permission.response)
    }

    fn remove_permission(&self, id: u64) {
        self.permissions
            .lock()
            .retain(|permission| permission.id != id);
    }

    fn wait_for_permission_response(&self, id: u64) -> AxResult {
        let response = loop {
            if self.is_destroyed() {
                self.remove_permission(id);
                return Ok(());
            }

            if let Some(response) = self.permission_response(id) {
                break response;
            }

            listener!(self.permission_event => listener);
            if self.is_destroyed() {
                self.remove_permission(id);
                return Ok(());
            }
            if let Some(response) = self.permission_response(id) {
                break response;
            }
            block_on(listener);
        };

        self.remove_permission(id);
        if response == FAN_ALLOW {
            Ok(())
        } else {
            Err(AxError::from(LinuxError::EPERM))
        }
    }

    fn event_fd(&self, event: &FanotifyEvent) -> AxResult<c_int> {
        if self.reports_fid() {
            return Ok(FAN_NOFD);
        }

        let Some(event_file) = event.file.as_ref() else {
            return Ok(FAN_NOFD);
        };

        let Some(path) = event.path.as_deref() else {
            return add_file_like(
                Arc::new(FanotifyEventFile::new(event_file.clone())),
                self.event_f_flags & O_CLOEXEC != 0,
            );
        };

        let file = open_fanotify_event_file(path, self.event_f_flags)
            .unwrap_or_else(|_| event_file.clone());
        add_file_like(
            Arc::new(FanotifyEventFile::new(file)),
            self.event_f_flags & O_CLOEXEC != 0,
        )
    }

    fn event_pidfd(&self, event: &FanotifyEvent) -> AxResult<Option<c_int>> {
        if !self.reports_pidfd() {
            return Ok(None);
        }

        if event.mask == FAN_Q_OVERFLOW {
            return Ok(Some(FAN_NOPIDFD));
        }

        let Ok(proc_data) = get_process_data(event.pid as u32) else {
            return Ok(Some(FAN_NOPIDFD));
        };
        Ok(Some(
            add_file_like(Arc::new(PidFd::new_process(&proc_data)), true).unwrap_or(FAN_NOPIDFD),
        ))
    }
}

fn close_fanotify_record_fds(fd: c_int, pidfd: Option<c_int>) {
    if fd >= 0 {
        let _ = close_file_like(fd);
    }
    if let Some(pidfd) = pidfd {
        if pidfd >= 0 {
            let _ = close_file_like(pidfd);
        }
    }
}

fn queue_overflow_event(events: &mut VecDeque<FanotifyEvent>, pid: i32) {
    if events.iter().any(|event| event.mask == FAN_Q_OVERFLOW) {
        return;
    }

    while events.len() >= FANOTIFY_MAX_QUEUED_EVENTS {
        events.pop_back();
    }
    events.push_back(FanotifyEvent {
        mask: FAN_Q_OVERFLOW,
        file: None,
        path: None,
        name: None,
        fid: None,
        fid_info_type: FANOTIFY_EVENT_INFO_TYPE_FID,
        child_fid: None,
        child_info_type: FANOTIFY_EVENT_INFO_TYPE_FID,
        child_name: None,
        pid,
        permission_id: None,
    });
}

fn fanotify_insert_before_pending_close(
    events: &VecDeque<FanotifyEvent>,
    event: &FanotifyEvent,
) -> Option<usize> {
    if event.mask & (FAN_DELETE | FAN_DELETE_SELF | FAN_MOVE_SELF | FAN_RENAME) == 0 {
        return None;
    }
    events.iter().position(|queued| {
        queued.pid == event.pid
            && queued.mask & FAN_CLOSE != 0
            && queued.mask & FAN_ONDIR == 0
            && (queued.fid == event.fid || queued.child_fid == event.child_fid)
    })
}

fn fanotify_can_merge_masks(
    queued_mask: u64,
    new_mask: u64,
    reports_name: bool,
    reports_target_fid: bool,
) -> bool {
    let open_close = FAN_OPEN | FAN_CLOSE;
    let self_events = FAN_DELETE_SELF | FAN_MOVE_SELF;
    let ondir = FAN_ONDIR;
    if queued_mask & ondir != 0
        && new_mask & ondir != 0
        && ((queued_mask & open_close != 0 && new_mask & self_events != 0)
            || (new_mask & open_close != 0 && queued_mask & self_events != 0))
    {
        return false;
    }

    if !reports_name || reports_target_fid {
        return true;
    }
    let close = FAN_CLOSE;
    let dirent = FAN_CREATE | FAN_DELETE | FAN_MOVE;
    !((queued_mask & close != 0 && new_mask & dirent != 0)
        || (new_mask & close != 0 && queued_mask & dirent != 0))
}

fn mark_matches_event_mask(
    mark: &FanotifyMark,
    target: FanotifyTarget,
    path: Option<&str>,
    mark_mask: u64,
    event_mask: u64,
    event_is_dir: bool,
) -> bool {
    if mark_mask == 0 || event_is_dir && mark_mask & FAN_ONDIR == 0 {
        return false;
    }

    match mark.kind {
        FanotifyMarkKind::Inode => {
            let self_match = mark.target == target
                || mark
                    .path
                    .as_deref()
                    .zip(path)
                    .is_some_and(|(mark_path, event_path)| mark_path == event_path);
            if event_mask & (FAN_DELETE_SELF | FAN_MOVE_SELF) != 0 {
                return self_match;
            }
            self_match || mark_matches_child(mark, path, mark_mask)
        }
        FanotifyMarkKind::Mount => mark
            .path
            .as_deref()
            .zip(path)
            .is_some_and(|(mark_path, event_path)| path_is_same_or_below(event_path, mark_path)),
        FanotifyMarkKind::Filesystem => mark.target.dev == target.dev,
    }
}

fn mark_matches_child(mark: &FanotifyMark, path: Option<&str>, mark_mask: u64) -> bool {
    mark_mask & FAN_EVENT_ON_CHILD != 0 && mark.target.is_dir && mark_is_parent_of_path(mark, path)
}

fn mark_ignores_rename(mark: &FanotifyMark, target: FanotifyTarget, path: Option<&str>) -> bool {
    if mark.ignored_mask & FAN_RENAME == 0 {
        return false;
    }

    match mark.kind {
        FanotifyMarkKind::Inode => {
            mark.target == target
                || mark
                    .path
                    .as_deref()
                    .zip(path)
                    .is_some_and(|(mark_path, event_path)| {
                        mark_path == event_path || parent_path(event_path) == Some(mark_path)
                    })
        }
        FanotifyMarkKind::Mount => mark
            .path
            .as_deref()
            .zip(path)
            .is_some_and(|(mark_path, event_path)| path_is_same_or_below(event_path, mark_path)),
        FanotifyMarkKind::Filesystem => mark.target.dev == target.dev,
    }
}

fn mark_ignores_child_move(
    mark: &FanotifyMark,
    target: FanotifyTarget,
    path: Option<&str>,
) -> bool {
    if mark.ignored_mask & FAN_MOVE == 0 {
        return false;
    }

    match mark.kind {
        FanotifyMarkKind::Inode => {
            mark.target == target
                || mark
                    .path
                    .as_deref()
                    .zip(path)
                    .is_some_and(|(mark_path, event_path)| {
                        parent_path(event_path) == Some(mark_path)
                    })
        }
        FanotifyMarkKind::Mount => mark
            .path
            .as_deref()
            .zip(path)
            .is_some_and(|(mark_path, event_path)| path_is_same_or_below(event_path, mark_path)),
        FanotifyMarkKind::Filesystem => mark.target.dev == target.dev,
    }
}

fn mark_is_parent_of_path(mark: &FanotifyMark, path: Option<&str>) -> bool {
    mark.path
        .as_deref()
        .zip(path)
        .and_then(|(mark_path, event_path)| {
            parent_path(event_path).map(|parent| (mark_path, parent))
        })
        .is_some_and(|(mark_path, parent)| mark_path == parent)
}

fn fanotify_event_name(mark: &FanotifyMark, path: Option<&str>) -> Option<(String, u8)> {
    let event_path = path?;
    let mark_path = mark.path.as_deref()?;
    if mark_is_parent_of_path(mark, path) {
        return Some((basename(event_path).to_string(), 2));
    }
    if event_path == mark_path {
        return Some((".".to_string(), 1));
    }
    Some((basename(event_path).to_string(), 0))
}

fn basename(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn parent_path(path: &str) -> Option<&str> {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
}

fn fanotify_parent_target_from_path(path: &str) -> Option<FanotifyTarget> {
    let parent = parent_path(path).unwrap_or(".");
    resolve_at(linux_raw_sys::general::AT_FDCWD, Some(parent), 0)
        .ok()
        .and_then(|resolved| resolved.stat().ok())
        .map(target_from_stat)
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
                let event_pidfd = self.event_pidfd(&event)?;
                if let Some(permission_id) = event.permission_id {
                    self.set_permission_fd(permission_id, event_fd);
                }
                let record = fanotify_event_record(&event, event_fd, event_pidfd);
                if dst.remaining_mut() < record.len() {
                    close_fanotify_record_fds(event_fd, event_pidfd);
                    self.events.lock().push_front(event);
                    return if written == 0 {
                        Err(AxError::InvalidInput)
                    } else {
                        Ok(written)
                    };
                }

                match dst.write(&record) {
                    Ok(0) => {
                        close_fanotify_record_fds(event_fd, event_pidfd);
                        self.events.lock().push_front(event);
                        break;
                    }
                    Ok(n) => written += n,
                    Err(err) => {
                        close_fanotify_record_fds(event_fd, event_pidfd);
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

    fn write(&self, src: &mut IoSrc) -> AxResult<usize> {
        if src.remaining() < FANOTIFY_RESPONSE_LEN {
            return Err(AxError::InvalidInput);
        }

        let mut buf = [0u8; FANOTIFY_RESPONSE_LEN];
        src.read_exact(&mut buf)?;
        let fd = c_int::from_ne_bytes(buf[0..4].try_into().unwrap());
        let response = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
        if response != FAN_ALLOW && response != FAN_DENY {
            return Err(AxError::InvalidInput);
        }

        self.set_permission_response(fd, response)?;
        Ok(FANOTIFY_RESPONSE_LEN)
    }

    fn proc_fdinfo(&self) -> Option<String> {
        let mut out = String::from("pos:\t0\nflags:\t0\nmnt_id:\t0\n");
        for mark in self.marks.lock().iter() {
            out.push_str(&format!(
                "fanotify ino:{:x} sdev:{:x} mflags: {:x} mask:{:x} ignored_mask:{:x}\n",
                mark.target.ino,
                mark.target.dev,
                fanotify_mark_fdinfo_flags(mark.kind),
                mark.mask,
                mark.ignored_mask,
            ));
        }
        Some(out)
    }

    fn nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }

    fn set_nonblocking(&self, nonblocking: bool) -> AxResult {
        self.nonblocking.store(nonblocking, Ordering::Release);
        Ok(())
    }
}

struct FanotifyEventFile {
    inner: Arc<dyn FileLike>,
}

impl FanotifyEventFile {
    fn new(inner: Arc<dyn FileLike>) -> Self {
        Self { inner }
    }
}

impl FileLike for FanotifyEventFile {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        self.inner.read(dst)
    }

    fn write(&self, src: &mut IoSrc) -> AxResult<usize> {
        self.inner.write(src)
    }

    fn stat(&self) -> AxResult<crate::file::Kstat> {
        self.inner.stat()
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> AxResult<usize> {
        self.inner.ioctl(cmd, arg)
    }

    fn set_nonblocking(&self, flag: bool) -> AxResult {
        self.inner.set_nonblocking(flag)
    }

    fn nonblocking(&self) -> bool {
        self.inner.nonblocking()
    }

    fn status_flags(&self) -> u32 {
        self.inner.status_flags()
    }

    fn set_status_flags(&self, flags: u32) -> AxResult {
        self.inner.set_status_flags(flags)
    }

    fn path(&self) -> Cow<'_, str> {
        self.inner.path()
    }
}

impl Pollable for FanotifyEventFile {
    fn poll(&self) -> IoEvents {
        self.inner.poll()
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        self.inner.register(context, events);
    }
}

fn fanotify_mark_fdinfo_flags(kind: FanotifyMarkKind) -> u32 {
    match kind {
        FanotifyMarkKind::Inode => 0,
        FanotifyMarkKind::Mount => FAN_MARK_MOUNT,
        FanotifyMarkKind::Filesystem => FAN_MARK_FILESYSTEM,
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

    let mut fds = FANOTIFY_FDS.write();
    fds.retain(|weak| weak.upgrade().is_some_and(|fd| !fd.is_destroyed()));
    if fds.len() >= FANOTIFY_MAX_USER_GROUPS {
        return Err(AxError::TooManyOpenFiles);
    }

    let fd = Arc::new(FanotifyFd::new(flags, event_f_flags));
    fd.set_nonblocking(flags & FAN_NONBLOCK != 0)?;
    fds.push(Arc::downgrade(&fd));
    drop(fds);
    add_file_like(fd, flags & FAN_CLOEXEC != 0).map(|fd| fd as isize)
}

pub(super) fn notify_fanotify_fd_close(file: &Arc<dyn FileLike>) {
    if let Some(fd) = file.downcast_ref::<FanotifyFd>() {
        fd.destroy_on_owner_close();
    }
}

pub(crate) fn evict_fanotify_marks() {
    let mut fds = FANOTIFY_FDS.write();
    fds.retain(|weak| {
        let Some(fd) = weak.upgrade() else {
            return false;
        };
        if fd.is_destroyed() {
            return false;
        }
        fd.marks.lock().retain(|mark| !mark.evictable);
        true
    });
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
    fd.apply_mark(flags, mask, target)?;
    Ok(0)
}

fn has_fanotify_interest(mask: u64) -> bool {
    FANOTIFY_FDS.read().iter().any(|weak| {
        weak.upgrade().is_some_and(|fd| {
            if fd.is_destroyed() {
                return false;
            }
            fd.marks
                .lock()
                .iter()
                .any(|mark| (mark.mask | mark.ignored_mask) & mask != 0)
        })
    })
}

fn notify_fanotify_target_event(
    target: FanotifyTarget,
    path: Option<String>,
    file: Arc<dyn FileLike>,
    child_target: Option<FanotifyTarget>,
    child_info_type: u8,
    child_name: Option<String>,
    child_path: Option<String>,
    object_target: Option<FanotifyTarget>,
    mask: u64,
    event_is_dir: bool,
) {
    if !has_fanotify_interest(mask) {
        return;
    }

    if file.downcast_ref::<FanotifyFd>().is_some()
        || file.downcast_ref::<FanotifyEventFile>().is_some()
    {
        return;
    }

    let curr = current();
    let tgid = curr.as_thread().proc_data.proc.pid() as i32;
    let tid = curr.id().as_u64() as i32;
    let mut fds = FANOTIFY_FDS.write();
    fds.retain(|weak| {
        let Some(fd) = weak.upgrade() else {
            return false;
        };
        if fd.is_destroyed() {
            return false;
        }
        fd.queue_event(
            target,
            path.as_deref(),
            file.clone(),
            child_target,
            child_info_type,
            child_name.as_deref(),
            child_path.as_deref(),
            object_target,
            tgid,
            tid,
            mask,
            event_is_dir,
            None,
        );
        true
    });
}

fn request_fanotify_target_permission(
    target: FanotifyTarget,
    path: Option<String>,
    file: Arc<dyn FileLike>,
    child_target: Option<FanotifyTarget>,
    child_info_type: u8,
    child_name: Option<String>,
    child_path: Option<String>,
    object_target: Option<FanotifyTarget>,
    mask: u64,
    event_is_dir: bool,
) -> AxResult {
    if !has_fanotify_interest(mask) {
        return Ok(());
    }

    if file.downcast_ref::<FanotifyFd>().is_some()
        || file.downcast_ref::<FanotifyEventFile>().is_some()
    {
        return Ok(());
    }

    let curr = current();
    let tgid = curr.as_thread().proc_data.proc.pid() as i32;
    let tid = curr.id().as_u64() as i32;
    let mut requests = Vec::new();
    let mut fds = FANOTIFY_FDS.write();
    fds.retain(|weak| {
        let Some(fd) = weak.upgrade() else {
            return false;
        };
        if fd.is_destroyed() {
            return false;
        }
        if let Some(id) = fd.queue_permission_event(
            target,
            path.as_deref(),
            file.clone(),
            child_target,
            child_info_type,
            child_name.as_deref(),
            child_path.as_deref(),
            object_target,
            tgid,
            tid,
            mask,
            event_is_dir,
        ) {
            requests.push((fd.clone(), id));
        }
        true
    });
    drop(fds);

    for (fd, id) in requests {
        fd.wait_for_permission_response(id)?;
    }
    Ok(())
}

fn notify_fanotify_event(file: Arc<dyn FileLike>, mask: u64) {
    let Ok(target) = target_from_file_like(&file) else {
        return;
    };
    let path = path_from_file_like(&file);
    notify_fanotify_target_event(
        target,
        path,
        file,
        None,
        FANOTIFY_EVENT_INFO_TYPE_FID,
        None,
        None,
        None,
        mask,
        target.is_dir,
    );
}

fn request_fanotify_permission(file: Arc<dyn FileLike>, mask: u64) -> AxResult {
    let target = target_from_file_like(&file)?;
    let path = path_from_file_like(&file);
    request_fanotify_target_permission(
        target,
        path,
        file,
        None,
        FANOTIFY_EVENT_INFO_TYPE_FID,
        None,
        None,
        None,
        mask,
        target.is_dir,
    )
}

fn fanotify_snapshot_file(path: String) -> Arc<dyn FileLike> {
    Arc::new(FanotifyPathFile { path })
}

fn notify_fanotify_resolved_path(dirfd: c_int, path: &str, mask: u64) {
    if let Some(event) = prepare_fanotify_path_event(dirfd, path, mask) {
        notify_fanotify_prepared_event(event);
    }
}

pub(super) fn prepare_fanotify_path_event(
    dirfd: c_int,
    path: &str,
    mask: u64,
) -> Option<FanotifyPreparedEvent> {
    if !has_fanotify_interest(mask) {
        return None;
    }
    let Ok(resolved) = resolve_at(dirfd, Some(path), 0) else {
        return None;
    };
    let Ok(target) = resolved.stat().map(target_from_stat) else {
        return None;
    };
    let path = resolved
        .into_file()
        .and_then(|loc| loc.absolute_path().ok())
        .map_or_else(|| path.to_string(), |path| path.to_string());
    Some(FanotifyPreparedEvent {
        target,
        child_target: None,
        child_info_type: FANOTIFY_EVENT_INFO_TYPE_FID,
        child_name: None,
        child_path: None,
        object_target: None,
        path: path.clone(),
        file: fanotify_snapshot_file(path),
        mask,
        event_is_dir: target.is_dir,
    })
}

pub(super) fn fanotify_prepared_event_is_dir(event: &FanotifyPreparedEvent) -> bool {
    event.event_is_dir
}

pub(super) fn fanotify_path_is_dir(dirfd: c_int, path: &str) -> Option<bool> {
    resolve_at(dirfd, Some(path), 0)
        .ok()
        .and_then(|resolved| resolved.stat().ok())
        .map(target_from_stat)
        .map(|target| target.is_dir)
}

pub(super) fn notify_fanotify_prepared_event(event: FanotifyPreparedEvent) {
    let target = event.target;
    let mask = event.mask;
    notify_fanotify_target_event(
        event.target,
        Some(event.path),
        event.file,
        event.child_target,
        event.child_info_type,
        event.child_name,
        event.child_path,
        event.object_target,
        event.mask,
        event.event_is_dir,
    );
    if mask & FAN_DELETE_SELF != 0 {
        forget_fanotify_path_alias(target);
    }
}

fn request_fanotify_prepared_permission(event: FanotifyPreparedEvent) -> AxResult {
    request_fanotify_target_permission(
        event.target,
        Some(event.path),
        event.file,
        event.child_target,
        event.child_info_type,
        event.child_name,
        event.child_path,
        event.object_target,
        event.mask,
        event.event_is_dir,
    )
}

pub(super) fn prepare_fanotify_dirent_event(
    dirfd: c_int,
    path: &str,
    mask: u64,
    event_is_dir: bool,
) -> Option<FanotifyPreparedEvent> {
    if !has_fanotify_interest(mask) {
        return None;
    }

    let Ok((parent, name)) = with_fs(dirfd, |fs| {
        fs.resolve_parent(Path::new(path))
            .map(|(parent, name)| (parent, name.to_string()))
    }) else {
        return None;
    };
    let child_target = resolve_at(dirfd, Some(path), 0)
        .ok()
        .and_then(|resolved| resolved.stat().ok())
        .map(target_from_stat);
    let parent_path = parent.absolute_path().ok().map(|path| path.to_string());
    let path = parent_path.map_or_else(
        || path.to_string(),
        |parent_path| {
            if parent_path == "/" {
                format!("/{name}")
            } else {
                format!("{parent_path}/{name}")
            }
        },
    );
    let file: Arc<dyn FileLike> = Arc::new(Directory::new(parent));
    let Ok(target) = target_from_file_like(&file) else {
        return None;
    };
    Some(FanotifyPreparedEvent {
        target,
        child_target,
        child_info_type: FANOTIFY_EVENT_INFO_TYPE_FID,
        child_name: None,
        child_path: None,
        object_target: None,
        path,
        file,
        mask,
        event_is_dir,
    })
}

fn notify_fanotify_dirent_event(dirfd: c_int, path: &str, mask: u64, event_is_dir: bool) {
    if let Some(event) = prepare_fanotify_dirent_event(dirfd, path, mask, event_is_dir) {
        notify_fanotify_prepared_event(event);
    }
}

pub(super) fn notify_fanotify_create(dirfd: c_int, path: &str, is_dir: bool) {
    notify_fanotify_dirent_event(dirfd, path, FAN_CREATE, is_dir);
}

pub(super) fn notify_fanotify_delete(dirfd: c_int, path: &str, is_dir: bool) {
    notify_fanotify_dirent_event(dirfd, path, FAN_DELETE, is_dir);
}

pub(super) fn prepare_fanotify_delete_event(
    dirfd: c_int,
    path: &str,
    is_dir: bool,
) -> Option<FanotifyPreparedEvent> {
    prepare_fanotify_dirent_event(dirfd, path, FAN_DELETE, is_dir)
}

pub(super) fn notify_fanotify_move_from(dirfd: c_int, path: &str, is_dir: bool) {
    notify_fanotify_dirent_event(dirfd, path, FAN_MOVED_FROM, is_dir);
}

pub(super) fn prepare_fanotify_move_from_event(
    dirfd: c_int,
    path: &str,
    is_dir: bool,
) -> Option<FanotifyPreparedEvent> {
    prepare_fanotify_dirent_event(dirfd, path, FAN_MOVED_FROM, is_dir)
}

pub(super) fn notify_fanotify_move_to(dirfd: c_int, path: &str, is_dir: bool) {
    notify_fanotify_dirent_event(dirfd, path, FAN_MOVED_TO, is_dir);
}

pub(super) fn prepare_fanotify_rename_event(
    old_dirfd: c_int,
    old_path: &str,
    new_dirfd: c_int,
    new_path: &str,
    is_dir: bool,
) -> Option<FanotifyPreparedEvent> {
    let mut event = prepare_fanotify_dirent_event(old_dirfd, old_path, FAN_RENAME, is_dir)?;
    event.object_target = event.child_target;
    let Ok((new_parent, new_name)) = with_fs(new_dirfd, |fs| {
        fs.resolve_parent(Path::new(new_path))
            .map(|(parent, name)| (parent, name.to_string()))
    }) else {
        return Some(event);
    };
    let new_parent_path = new_parent.absolute_path().ok().map(|path| path.to_string());
    let new_child_path = new_parent_path.as_ref().map(|parent_path| {
        if parent_path == "/" {
            format!("/{new_name}")
        } else {
            format!("{parent_path}/{new_name}")
        }
    });
    let file: Arc<dyn FileLike> = Arc::new(Directory::new(new_parent));
    if let Ok(new_parent_target) = target_from_file_like(&file) {
        event.child_target = Some(new_parent_target);
        event.child_info_type = FANOTIFY_EVENT_INFO_TYPE_NEW_DFID_NAME;
        event.child_name = Some(new_name);
        event.child_path = new_child_path;
        event.mask = FAN_RENAME;
    }
    Some(event)
}

pub(super) fn notify_fanotify_delete_self(dirfd: c_int, path: &str) {
    notify_fanotify_resolved_path(dirfd, path, FAN_DELETE_SELF);
}

pub(super) fn prepare_fanotify_delete_self_event(
    dirfd: c_int,
    path: &str,
) -> Option<FanotifyPreparedEvent> {
    prepare_fanotify_path_event(dirfd, path, FAN_DELETE_SELF)
}

pub(super) fn prepare_fanotify_move_self_event(
    dirfd: c_int,
    path: &str,
) -> Option<FanotifyPreparedEvent> {
    prepare_fanotify_path_event(dirfd, path, FAN_MOVE_SELF)
}

pub(super) fn notify_fanotify_modify_path(dirfd: c_int, path: &str) {
    notify_fanotify_resolved_path(dirfd, path, FAN_MODIFY);
}

pub(super) fn notify_fanotify_open(file: Arc<dyn FileLike>) {
    notify_fanotify_event(file, FAN_OPEN);
}

pub(super) fn check_fanotify_open_permission(file: Arc<dyn FileLike>) -> AxResult {
    request_fanotify_permission(file, FAN_OPEN_PERM)
}

pub(crate) fn notify_fanotify_open_exec_path(path: &str) {
    if !has_fanotify_interest(FAN_OPEN | FAN_OPEN_EXEC) {
        return;
    }

    if let Ok(file) = open_fanotify_event_file(path, 0) {
        notify_fanotify_event(file, FAN_OPEN | FAN_OPEN_EXEC);
    }
}

pub(crate) fn check_fanotify_open_permission_path(path: &str) -> AxResult {
    if let Some(event) =
        prepare_fanotify_path_event(linux_raw_sys::general::AT_FDCWD, path, FAN_OPEN_PERM)
    {
        request_fanotify_prepared_permission(event)?;
    }
    Ok(())
}

pub(crate) fn check_fanotify_open_exec_permission_path(path: &str) -> AxResult {
    if let Some(event) =
        prepare_fanotify_path_event(linux_raw_sys::general::AT_FDCWD, path, FAN_OPEN_EXEC_PERM)
    {
        request_fanotify_prepared_permission(event)?;
    }
    Ok(())
}

pub(super) fn notify_fanotify_access(file: Arc<dyn FileLike>) {
    notify_fanotify_event(file, FAN_ACCESS);
}

pub(super) fn check_fanotify_access_permission(file: Arc<dyn FileLike>) -> AxResult {
    request_fanotify_permission(file, FAN_ACCESS_PERM)
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

fn fanotify_event_record(event: &FanotifyEvent, fd: c_int, pidfd: Option<c_int>) -> Vec<u8> {
    let fid = event.fid.filter(|_| event.mask != FAN_Q_OVERFLOW);
    let child_fid = event
        .child_fid
        .filter(|_| event.mask != FAN_Q_OVERFLOW)
        .map(|fid| fanotify_refresh_child_fid(event.path.as_deref(), fid));
    let child_name = event
        .child_name
        .as_deref()
        .filter(|_| event.mask != FAN_Q_OVERFLOW);
    let name = event
        .name
        .as_deref()
        .filter(|_| event.mask != FAN_Q_OVERFLOW);
    let info_len = fid.map_or(0, |_| {
        fanotify_event_info_len(name.map(|name| name.len() + 1))
    });
    let child_info_len = child_fid.map_or(0, |_| {
        fanotify_event_info_len(child_name.map(|name| name.len() + 1))
    });
    let pidfd_info_len = pidfd.map_or(0, |_| FANOTIFY_EVENT_INFO_PIDFD_LEN);
    let record_len = FANOTIFY_EVENT_METADATA_LEN + info_len + child_info_len + pidfd_info_len;
    let mut out = Vec::with_capacity(record_len);
    out.extend_from_slice(&fanotify_event_metadata(
        event.mask, fd, event.pid, record_len,
    ));

    if let Some(fid) = fid {
        append_fanotify_event_info(&mut out, event.fid_info_type, fid, name);
    }

    if let Some(child_fid) = child_fid {
        append_fanotify_event_info(&mut out, event.child_info_type, child_fid, child_name);
    }

    if let Some(pidfd) = pidfd {
        out.push(FANOTIFY_EVENT_INFO_TYPE_PIDFD);
        out.push(0);
        out.extend_from_slice(&(FANOTIFY_EVENT_INFO_PIDFD_LEN as u16).to_ne_bytes());
        out.extend_from_slice(&pidfd.to_ne_bytes());
    }

    out
}

fn fanotify_refresh_child_fid(path: Option<&str>, fid: FanotifyTarget) -> FanotifyTarget {
    if !fid.is_dir {
        return fid;
    }
    path.and_then(|path| resolve_at(linux_raw_sys::general::AT_FDCWD, Some(path), 0).ok())
        .and_then(|resolved| resolved.stat().ok())
        .map(target_from_stat)
        .unwrap_or(fid)
}

fn fanotify_event_info_len(name_len: Option<usize>) -> usize {
    align_to_8(
        FANOTIFY_EVENT_INFO_FID_HEADER_LEN
            + FANOTIFY_FILE_HANDLE_HEADER_LEN
            + FANOTIFY_FILE_HANDLE_BYTES
            + name_len.unwrap_or(0),
    )
}

fn append_fanotify_event_info(
    out: &mut Vec<u8>,
    info_type: u8,
    fid: FanotifyTarget,
    name: Option<&str>,
) {
    let info_len = fanotify_event_info_len(name.map(|name| name.len() + 1));
    out.push(info_type);
    out.push(0);
    out.extend_from_slice(&(info_len as u16).to_ne_bytes());
    let handle = fanotify_target_handle(fid);
    out.extend_from_slice(&0i32.to_ne_bytes());
    out.extend_from_slice(&(fid.dev as i32).to_ne_bytes());
    out.extend_from_slice(&(FANOTIFY_FILE_HANDLE_BYTES as u32).to_ne_bytes());
    out.extend_from_slice(&FANOTIFY_FILE_HANDLE_TYPE.to_ne_bytes());
    out.extend_from_slice(&handle);
    if let Some(name) = name {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    out.resize(out.len().next_multiple_of(8), 0);
}

fn align_to_8(value: usize) -> usize {
    (value + 7) & !7
}

fn fanotify_target_handle(target: FanotifyTarget) -> [u8; FANOTIFY_FILE_HANDLE_BYTES] {
    let mut handle = [0u8; FANOTIFY_FILE_HANDLE_BYTES];
    handle[..8].copy_from_slice(&target.dev.to_ne_bytes());
    handle[8..].copy_from_slice(&target.ino.to_ne_bytes());
    handle
}

fn fanotify_event_metadata(
    mask: u64,
    fd: c_int,
    pid: i32,
    event_len: usize,
) -> [u8; FANOTIFY_EVENT_METADATA_LEN] {
    let mut out = [0u8; FANOTIFY_EVENT_METADATA_LEN];
    out[0..4].copy_from_slice(&(event_len as u32).to_ne_bytes());
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
    if let Ok(target) = target_from_file_like(file) {
        if let Some(alias) = fanotify_path_alias(target) {
            return Some(alias);
        }
    }
    let path = file.path();
    if path.starts_with('<') {
        None
    } else {
        Some(path.to_string())
    }
}

fn fanotify_path_alias(target: FanotifyTarget) -> Option<String> {
    FANOTIFY_PATH_ALIASES
        .read()
        .iter()
        .rev()
        .find(|alias| alias.target == target)
        .map(|alias| alias.path.clone())
}

fn forget_fanotify_path_alias(target: FanotifyTarget) {
    FANOTIFY_PATH_ALIASES
        .write()
        .retain(|alias| alias.target != target);
}

pub(super) fn prepare_fanotify_path_alias(
    dirfd: c_int,
    old_path: &str,
    new_dirfd: c_int,
    new_path: &str,
) -> Option<FanotifyPathAlias> {
    let target = resolve_at(dirfd, Some(old_path), 0)
        .ok()
        .and_then(|resolved| resolved.stat().ok())
        .map(target_from_stat)?;
    let Ok((new_parent, new_name)) = with_fs(new_dirfd, |fs| {
        fs.resolve_parent(Path::new(new_path))
            .map(|(parent, name)| (parent, name.to_string()))
    }) else {
        return None;
    };
    let parent_path = new_parent
        .absolute_path()
        .ok()
        .map(|path| path.to_string())?;
    let path = if parent_path == "/" {
        format!("/{new_name}")
    } else {
        format!("{parent_path}/{new_name}")
    };
    Some(FanotifyPathAlias { target, path })
}

pub(super) fn commit_fanotify_path_alias(alias: Option<FanotifyPathAlias>) {
    if let Some(alias) = alias {
        let mut aliases = FANOTIFY_PATH_ALIASES.write();
        aliases.retain(|old| old.target != alias.target);
        aliases.push(alias);
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
