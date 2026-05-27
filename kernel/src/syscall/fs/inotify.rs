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
    sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult};
use axfs_ng_vfs::path::Path;
use axio::prelude::*;
use axpoll::{IoEvents, PollSet, Pollable};
use axsync::Mutex;
use axtask::{
    future::{block_on, poll_io},
    yield_now,
};
use linux_raw_sys::general::*;
use spin::RwLock;

use crate::{
    file::{Directory, FileLike, IoDst, add_file_like, resolve_at, with_fs},
    mm::vm_load_string,
};

const EVENT_HDR_LEN: usize = 16;
const WATCH_FLAGS: u32 =
    IN_ONLYDIR | IN_DONT_FOLLOW | IN_EXCL_UNLINK | IN_MASK_ADD | IN_MASK_CREATE | IN_ONESHOT;
const USER_EVENTS: u32 = IN_ALL_EVENTS | IN_UNMOUNT;
const VALID_INIT_FLAGS: u32 = IN_CLOEXEC | IN_NONBLOCK;
const VALID_WATCH_MASK: u32 = USER_EVENTS | WATCH_FLAGS;

static INOTIFY_FDS: RwLock<Vec<Weak<InotifyFd>>> = RwLock::new(Vec::new());
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);
static MAX_QUEUED_EVENTS: AtomicUsize = AtomicUsize::new(16384);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    dev: u64,
    ino: u64,
    is_dir: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct InotifyTarget(Target);

#[derive(Clone)]
struct Watch {
    wd: c_int,
    target: Target,
    mask: u32,
}

#[derive(PartialEq, Eq)]
struct Event {
    wd: c_int,
    mask: u32,
    cookie: u32,
    name: Option<String>,
}

pub struct InotifyFd {
    nonblocking: AtomicBool,
    next_wd: AtomicI32,
    watches: Mutex<Vec<Watch>>,
    events: Mutex<VecDeque<Event>>,
    poll: PollSet,
}

impl InotifyFd {
    fn new() -> Self {
        Self {
            nonblocking: AtomicBool::new(false),
            next_wd: AtomicI32::new(1),
            watches: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::new()),
            poll: PollSet::new(),
        }
    }

    fn push_event(&self, event: Event) {
        let mut events = self.events.lock();
        if events.back().is_some_and(|last| last == &event) {
            return;
        }
        let max = MAX_QUEUED_EVENTS.load(Ordering::Acquire).max(1);
        if events.len() >= max {
            if !events.back().is_some_and(|e| e.mask == IN_Q_OVERFLOW) {
                events.push_back(Event {
                    wd: -1,
                    mask: IN_Q_OVERFLOW,
                    cookie: 0,
                    name: None,
                });
            }
        } else {
            events.push_back(event);
        }
        self.poll.wake();
    }

    fn queue_matching(
        &self,
        target: Target,
        mask: u32,
        name: Option<&str>,
        event_is_dir: bool,
        cookie: u32,
        skip_excl_unlink: bool,
    ) {
        let self_event = mask & (IN_DELETE_SELF | IN_MOVE_SELF | IN_UNMOUNT) != 0;
        let event_mask = mask
            | if event_is_dir && !self_event {
                IN_ISDIR
            } else {
                0
            };
        let hits = self
            .watches
            .lock()
            .iter()
            .filter(|w| {
                w.target == target
                    && w.mask & mask != 0
                    && !(skip_excl_unlink && name.is_some() && w.mask & IN_EXCL_UNLINK != 0)
            })
            .map(|w| (w.wd, w.mask & IN_ONESHOT != 0))
            .collect::<Vec<_>>();
        for (wd, oneshot) in hits {
            self.push_event(Event {
                wd,
                mask: event_mask,
                cookie,
                name: name.map(ToString::to_string),
            });
            if oneshot {
                self.remove_watch(wd, true);
            }
        }
    }

    fn remove_target(&self, target: Target) {
        let wds = self
            .watches
            .lock()
            .iter()
            .filter(|w| w.target == target)
            .map(|w| w.wd)
            .collect::<Vec<_>>();
        for wd in wds {
            self.remove_watch(wd, true);
        }
    }

    fn unmount_dev(&self, dev: u64) {
        let wds = self
            .watches
            .lock()
            .iter()
            .filter(|w| w.target.dev == dev)
            .map(|w| w.wd)
            .collect::<Vec<_>>();
        for wd in wds {
            self.push_event(Event {
                wd,
                mask: IN_UNMOUNT,
                cookie: 0,
                name: None,
            });
            self.remove_watch(wd, true);
        }
    }

    fn remove_watch(&self, wd: c_int, notify: bool) -> bool {
        let mut watches = self.watches.lock();
        let Some(pos) = watches.iter().position(|w| w.wd == wd) else {
            return false;
        };
        watches.remove(pos);
        drop(watches);
        if notify {
            self.push_event(Event {
                wd,
                mask: IN_IGNORED,
                cookie: 0,
                name: None,
            });
        }
        true
    }
}

impl FileLike for InotifyFd {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        block_on(poll_io(self, IoEvents::IN, self.nonblocking(), || {
            let mut events = self.events.lock();
            if events.is_empty() {
                return Err(AxError::WouldBlock);
            }
            let mut written = 0;
            while let Some(event) = events.front() {
                let record = event_record(event);
                if record.len() > dst.remaining_mut() {
                    return if written == 0 {
                        Err(AxError::InvalidInput)
                    } else {
                        Ok(written)
                    };
                }
                dst.write_all(&record)?;
                written += record.len();
                events.pop_front();
            }
            Ok(written)
        }))
    }

    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[inotify]".into()
    }

    fn proc_fdinfo(&self) -> Option<String> {
        let mut out = String::from("pos:\t0\nflags:\t0\nmnt_id:\t0\n");
        for w in self.watches.lock().iter() {
            out.push_str(&format!(
                "inotify wd:{} ino:{:x} sdev:{:x} mask:{:x}\n",
                w.wd, w.target.ino, w.target.dev, w.mask
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

impl Pollable for InotifyFd {
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

fn event_record(event: &Event) -> Vec<u8> {
    let name_len = event
        .name
        .as_ref()
        .map_or(0, |name| (name.len() + 1 + 15) & !15);
    let mut out = Vec::with_capacity(EVENT_HDR_LEN + name_len);
    out.extend_from_slice(&event.wd.to_ne_bytes());
    out.extend_from_slice(&event.mask.to_ne_bytes());
    out.extend_from_slice(&event.cookie.to_ne_bytes());
    out.extend_from_slice(&(name_len as u32).to_ne_bytes());
    if let Some(name) = &event.name {
        out.extend_from_slice(name.as_bytes());
        out.resize(EVENT_HDR_LEN + name_len, 0);
    }
    out
}

fn target_from_stat(stat: crate::file::Kstat) -> Target {
    Target {
        dev: stat.dev,
        ino: stat.ino,
        is_dir: stat.mode & S_IFMT == S_IFDIR,
    }
}

fn target_from_file(file: &Arc<dyn FileLike>) -> AxResult<Target> {
    file.stat().map(target_from_stat)
}

fn target_from_path(dirfd: c_int, path: &str, flags: u32) -> AxResult<(Target, String)> {
    let resolve_flags = if flags & IN_DONT_FOLLOW != 0 {
        AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    let resolved = resolve_at(dirfd, Some(path), resolve_flags)?;
    let target = target_from_stat(resolved.stat()?);
    let path = resolved
        .into_file()
        .and_then(|loc| loc.absolute_path().ok())
        .map_or_else(|| path.to_string(), |p| p.to_string());
    Ok((target, path))
}

fn parent_target(dirfd: c_int, path: &str) -> Option<(Target, String)> {
    let (parent, name) = with_fs(dirfd, |fs| {
        fs.resolve_parent(Path::new(path))
            .map(|(parent, name)| (parent, name.to_string()))
    })
    .ok()?;
    let file: Arc<dyn FileLike> = Arc::new(Directory::new(parent));
    Some((target_from_file(&file).ok()?, name))
}

fn parent_from_file_path(path: &str) -> Option<(Target, String)> {
    let path = path.trim_end_matches('/');
    let split = path.rfind('/')?;
    let name = &path[split + 1..];
    if name.is_empty() {
        return None;
    }
    let parent = if split == 0 { "/" } else { &path[..split] };
    let resolved = resolve_at(AT_FDCWD, Some(parent), 0).ok()?;
    Some((target_from_stat(resolved.stat().ok()?), name.to_string()))
}

fn for_each_fd(mut f: impl FnMut(&Arc<InotifyFd>)) {
    let mut fds = INOTIFY_FDS.write();
    fds.retain(|weak| {
        let Some(fd) = weak.upgrade() else {
            return false;
        };
        f(&fd);
        true
    });
}

fn remove_target(target: Target) {
    for_each_fd(|fd| fd.remove_target(target));
}

fn notify_target(
    target: Target,
    mask: u32,
    name: Option<&str>,
    is_dir: bool,
    cookie: u32,
    skip_excl_unlink: bool,
) {
    for_each_fd(|fd| fd.queue_matching(target, mask, name, is_dir, cookie, skip_excl_unlink));
}

pub fn notify_inotify_file(file: Arc<dyn FileLike>, mask: u32) {
    let Ok(stat) = file.stat() else {
        return;
    };
    let target = target_from_stat(stat);
    if let Some((parent, name)) = parent_from_file_path(file.path().as_ref()) {
        notify_target(parent, mask, Some(&name), target.is_dir, 0, stat.nlink == 0);
    }
    notify_target(target, mask, None, target.is_dir, 0, false);
}

pub fn notify_inotify_path(dirfd: c_int, path: &str, mask: u32) {
    let Ok((target, _)) = target_from_path(dirfd, path, 0) else {
        return;
    };
    if mask & (IN_DELETE_SELF | IN_MOVE_SELF | IN_UNMOUNT) == 0
        && let Some((parent, name)) = parent_target(dirfd, path)
    {
        notify_target(parent, mask, Some(&name), target.is_dir, 0, false);
    }
    notify_target(target, mask, None, target.is_dir, 0, false);
    if mask & IN_DELETE_SELF != 0 {
        remove_target(target);
    }
}

pub fn notify_inotify_dirent(dirfd: c_int, path: &str, mask: u32, is_dir: bool, cookie: u32) {
    if let Some((parent, name)) = parent_target(dirfd, path) {
        notify_target(parent, mask, Some(&name), is_dir, cookie, false);
        if mask == IN_DELETE {
            yield_now();
        }
    }
}

pub(crate) fn prepare_inotify_target(dirfd: c_int, path: &str) -> Option<InotifyTarget> {
    target_from_path(dirfd, path, 0)
        .ok()
        .map(|(target, _)| InotifyTarget(target))
}

pub(crate) fn notify_inotify_target(target: InotifyTarget, mask: u32) {
    notify_target(target.0, mask, None, target.0.is_dir, 0, false);
    if mask & IN_DELETE_SELF != 0 {
        remove_target(target.0);
    }
}

pub fn notify_inotify_unmount_dev(dev: u64) {
    for_each_fd(|fd| fd.unmount_dev(dev));
}

pub fn next_inotify_cookie() -> u32 {
    NEXT_COOKIE.fetch_add(1, Ordering::AcqRel).max(1)
}

pub fn inotify_max_queued_events() -> usize {
    MAX_QUEUED_EVENTS.load(Ordering::Acquire)
}

pub fn set_inotify_max_queued_events(value: usize) {
    MAX_QUEUED_EVENTS.store(value.max(1), Ordering::Release);
}

pub fn sys_inotify_init1(flags: u32) -> AxResult<isize> {
    if flags & !VALID_INIT_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }
    let fd = Arc::new(InotifyFd::new());
    fd.set_nonblocking(flags & IN_NONBLOCK != 0)?;
    INOTIFY_FDS.write().push(Arc::downgrade(&fd));
    add_file_like(fd, flags & IN_CLOEXEC != 0).map(|fd| fd as _)
}

pub fn sys_inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> AxResult<isize> {
    if mask & !VALID_WATCH_MASK != 0
        || mask & USER_EVENTS == 0
        || mask & (IN_MASK_ADD | IN_MASK_CREATE) == (IN_MASK_ADD | IN_MASK_CREATE)
    {
        return Err(AxError::InvalidInput);
    }
    let inotify = InotifyFd::from_fd(fd)?;
    let path = vm_load_string(path)?;
    let (target, _) = target_from_path(AT_FDCWD, &path, mask)?;
    if mask & IN_ONLYDIR != 0 && !target.is_dir {
        return Err(AxError::NotADirectory);
    }

    let mut watches = inotify.watches.lock();
    if let Some(w) = watches.iter_mut().find(|w| w.target == target) {
        if mask & IN_MASK_CREATE != 0 {
            return Err(AxError::AlreadyExists);
        }
        w.mask = if mask & IN_MASK_ADD != 0 {
            w.mask | mask
        } else {
            mask
        };
        return Ok(w.wd as _);
    }
    let wd = inotify.next_wd.fetch_add(1, Ordering::AcqRel);
    watches.push(Watch { wd, target, mask });
    Ok(wd as _)
}

pub fn sys_inotify_rm_watch(fd: c_int, wd: c_int) -> AxResult<isize> {
    let inotify = InotifyFd::from_fd(fd)?;
    if inotify.remove_watch(wd, true) {
        Ok(0)
    } else {
        Err(AxError::InvalidInput)
    }
}
