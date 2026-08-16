// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2025 KylinSoft Co., Ltd. <https://www.kylinos.cn/>
// Copyright (C) 2025 Azure-stars <Azure_stars@126.com>
// Copyright (C) 2025 Yuekai Jia <equation618@gmail.com>
// See LICENSES for license details.
//
// This file has been modified by KylinSoft on 2025.

use alloc::{
    borrow::Cow,
    collections::vec_deque::VecDeque,
    sync::{Arc, Weak},
    task::Wake,
    vec::Vec,
};
use core::{
    hash::{Hash, Hasher},
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Waker},
};

use axerrno::{AxError, AxResult};
use axpoll::{IoEvents, PollSet, Pollable};
use bitflags::bitflags;
use hashbrown::HashMap;
use kspin::SpinNoPreempt;
use linux_raw_sys::general::{EPOLLET, EPOLLONESHOT, epoll_event};

use crate::file::{Directory, FileLike, get_file_like};

pub struct EpollEvent {
    pub events: IoEvents,
    pub user_data: u64,
}

bitflags! {
    /// Flags for the entries in the `epoll` instance.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct EpollFlags: u32 {
        const EDGE_TRIGGER = EPOLLET;
        const ONESHOT = EPOLLONESHOT;
    }
}

/// Interest trigger mode
#[derive(Debug, Clone, Copy)]
enum TriggerMode {
    /// Level-triggered: until the condition is cleared
    Level,
    /// Edge-triggered: only notify when the condition changes
    Edge,
    /// One-shot: notify only once
    OneShot { fired: bool },
}

impl TriggerMode {
    fn from_flags(flags: EpollFlags) -> Self {
        if flags.contains(EpollFlags::ONESHOT) {
            TriggerMode::OneShot { fired: false }
        } else if flags.contains(EpollFlags::EDGE_TRIGGER) {
            TriggerMode::Edge
        } else {
            TriggerMode::Level
        }
    }

    // return should notify and new mode
    fn should_notify(&self) -> (bool, Self) {
        match self {
            TriggerMode::Level => {
                // LT: always notify
                (true, *self)
            }
            // if we could wake, we need notify
            TriggerMode::Edge => (true, TriggerMode::Edge),
            TriggerMode::OneShot { fired } => {
                // ONESHOT: 只触发一次
                if *fired {
                    (false, *self)
                } else {
                    (true, TriggerMode::OneShot { fired: true })
                }
            }
        }
    }

    fn is_enabled(&self) -> bool {
        match self {
            TriggerMode::OneShot { fired } => !fired,
            _ => true,
        }
    }
}

enum ConsumeResult {
    // success and should keep in ready list
    EventAndKeep(EpollEvent),
    // success and hould remove ready list
    EventAndRemove(EpollEvent),
    // no event and should remove ready list
    NoEvent,
}

#[derive(Clone)]
struct EntryKey {
    fd: i32,
    file: Weak<dyn FileLike>,
}
impl EntryKey {
    fn new(fd: i32) -> AxResult<Self> {
        let file = get_file_like(fd)?;
        Ok(Self {
            fd,
            file: Arc::downgrade(&file),
        })
    }

    #[inline]
    fn get_file(&self) -> Option<Arc<dyn FileLike>> {
        self.file.upgrade()
    }
}

impl Hash for EntryKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.fd, self.file.as_ptr()).hash(state);
    }
}
impl PartialEq for EntryKey {
    fn eq(&self, other: &Self) -> bool {
        self.fd == other.fd && Weak::ptr_eq(&self.file, &other.file)
    }
}

impl Eq for EntryKey {}

struct EpollInterest {
    key: EntryKey,
    event: EpollEvent,
    mode: SpinNoPreempt<TriggerMode>,
    in_ready_queue: AtomicBool,
}

impl EpollInterest {
    fn new_shared(key: EntryKey, event: EpollEvent, flags: EpollFlags) -> Arc<Self> {
        let mut storage = Arc::<Self>::new_uninit();
        let dst = Arc::get_mut(&mut storage)
            .expect("new epoll interest allocation must be unique")
            .as_mut_ptr();
        unsafe {
            core::ptr::addr_of_mut!((*dst).key).write(key);
            core::ptr::addr_of_mut!((*dst).event).write(event);
            SpinNoPreempt::write_unpublished_in_place(
                core::ptr::addr_of_mut!((*dst).mode),
                TriggerMode::from_flags(flags),
            );
            core::ptr::addr_of_mut!((*dst).in_ready_queue).write(AtomicBool::new(false));
            storage.assume_init()
        }
    }

    #[inline]
    fn is_enabled(&self) -> bool {
        self.mode.lock().is_enabled()
    }

    #[inline]
    fn is_level_triggered(&self) -> bool {
        matches!(*self.mode.lock(), TriggerMode::Level)
    }

    #[inline]
    fn try_mark_in_queue(&self) -> bool {
        self.in_ready_queue
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    #[inline]
    fn mark_not_in_queue(&self) {
        self.in_ready_queue.store(false, Ordering::Release);
    }

    #[inline]
    fn monitored_events(&self) -> IoEvents {
        // EPOLLERR and EPOLLHUP are always monitored and reported by Linux,
        // even when user space did not include them in epoll_event.events.
        // Runtimes such as libuv rely on the implicit HUP to observe pipe EOF.
        self.event.events | IoEvents::ALWAYS_POLL
    }

    #[inline]
    fn ready_events(&self, file: &dyn FileLike) -> IoEvents {
        file.poll() & self.monitored_events()
    }

    fn consume(&self, file: &dyn FileLike) -> ConsumeResult {
        let matched = self.ready_events(file);

        // not ready
        if matched.is_empty() {
            return ConsumeResult::NoEvent;
        }

        let mut mode = self.mode.lock();
        let (should_notify, new_mode) = mode.should_notify();
        *mode = new_mode;
        trace!(
            "consume fd: {} matches {:?} should notify: {} ",
            self.key.fd, matched, should_notify
        );

        if !should_notify {
            return ConsumeResult::NoEvent;
        }

        // create event
        let event = EpollEvent {
            events: matched,
            user_data: self.event.user_data,
        };

        // shoud still keep in ready?
        match *mode {
            TriggerMode::Level => ConsumeResult::EventAndKeep(event),
            TriggerMode::Edge | TriggerMode::OneShot { .. } => ConsumeResult::EventAndRemove(event),
        }
    }
}

struct InterestWaker {
    epoll: Weak<EpollInner>,
    interest: Weak<EpollInterest>,
}

impl Wake for InterestWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let Some(epoll) = self.epoll.upgrade() else {
            return;
        };

        let Some(interest) = self.interest.upgrade() else {
            return;
        };

        if interest.try_mark_in_queue() {
            epoll
                .ready_queue
                .lock()
                .push_back(Arc::downgrade(&interest));
            trace!(
                "Epoll: fd={} added to ready queue, events={:?} wake up poller",
                interest.key.fd, interest.event.events
            );
            epoll.poll_ready.wake();
        }
    }
}

struct EpollInner {
    interests: SpinNoPreempt<HashMap<EntryKey, Arc<EpollInterest>>>,
    ready_queue: SpinNoPreempt<VecDeque<Weak<EpollInterest>>>,
    poll_ready: PollSet,
}

impl EpollInner {
    fn new_shared() -> Arc<Self> {
        let mut storage = Arc::<Self>::new_uninit();
        let dst = Arc::get_mut(&mut storage)
            .expect("new epoll allocation must be unique")
            .as_mut_ptr();
        unsafe {
            SpinNoPreempt::write_unpublished_in_place(
                core::ptr::addr_of_mut!((*dst).interests),
                HashMap::new(),
            );
            SpinNoPreempt::write_unpublished_in_place(
                core::ptr::addr_of_mut!((*dst).ready_queue),
                VecDeque::new(),
            );
            PollSet::write_unpublished_in_place(core::ptr::addr_of_mut!((*dst).poll_ready));
            storage.assume_init()
        }
    }
}

pub struct Epoll {
    inner: Arc<EpollInner>,
}

impl Default for Epoll {
    fn default() -> Self {
        Self {
            inner: EpollInner::new_shared(),
        }
    }
}

impl Epoll {
    pub fn new() -> Self {
        Self::default()
    }

    // only register waker, not add to ready queue
    fn register_waker_only(&self, interest: &Arc<EpollInterest>) {
        let Some(file) = interest.key.get_file() else {
            return;
        };

        if !interest.is_enabled() {
            return;
        }

        let waker = Waker::from(Arc::new(InterestWaker {
            epoll: Arc::downgrade(&self.inner),
            interest: Arc::downgrade(interest),
        }));

        let mut context = Context::from_waker(&waker);
        file.register(&mut context, interest.monitored_events());
    }

    // for add/modify
    fn check_and_register_waker(&self, interest: &Arc<EpollInterest>) {
        let Some(file) = interest.key.get_file() else {
            return;
        };

        if !interest.is_enabled() {
            return;
        }

        let waker = Waker::from(Arc::new(InterestWaker {
            epoll: Arc::downgrade(&self.inner),
            interest: Arc::downgrade(interest),
        }));

        let current = interest.ready_events(file.as_ref());

        if !current.is_empty() {
            waker.wake_by_ref();
        } else {
            let mut context = Context::from_waker(&waker);
            file.register(&mut context, interest.monitored_events());

            let current = interest.ready_events(file.as_ref());
            if !current.is_empty() {
                waker.wake_by_ref();
            }
        }
    }

    fn inner_ptr(&self) -> *const EpollInner {
        Arc::as_ptr(&self.inner)
    }

    fn contains_epoll_inner(
        &self,
        needle: *const EpollInner,
        seen: &mut Vec<*const EpollInner>,
    ) -> bool {
        let this = self.inner_ptr();
        if this == needle {
            return true;
        }
        if seen.contains(&this) {
            return false;
        }
        seen.push(this);

        self.inner.interests.lock().values().any(|interest| {
            interest
                .key
                .get_file()
                .and_then(|file| file.downcast_arc::<Epoll>().ok())
                .is_some_and(|epoll| epoll.contains_epoll_inner(needle, seen))
        })
    }

    fn nesting_depth(&self, seen: &mut Vec<*const EpollInner>) -> usize {
        let this = self.inner_ptr();
        if seen.contains(&this) {
            return 0;
        }
        seen.push(this);

        1 + self
            .inner
            .interests
            .lock()
            .values()
            .filter_map(|interest| interest.key.get_file())
            .filter_map(|file| file.downcast_arc::<Epoll>().ok())
            .map(|epoll| epoll.nesting_depth(seen))
            .max()
            .unwrap_or(0)
    }

    pub fn add(&self, fd: i32, event: EpollEvent, flags: EpollFlags) -> AxResult<()> {
        let key = EntryKey::new(fd)?;
        let file = key.get_file().ok_or(AxError::BadFileDescriptor)?;
        if file.downcast_ref::<Directory>().is_some() {
            return Err(AxError::OperationNotPermitted);
        }
        if let Some(epoll) = file.downcast_ref::<Epoll>() {
            if epoll.contains_epoll_inner(self.inner_ptr(), &mut Vec::new()) {
                return Err(AxError::FilesystemLoop);
            }
            if epoll.nesting_depth(&mut Vec::new()) >= 5 {
                return Err(AxError::InvalidInput);
            }
        }

        let interest = EpollInterest::new_shared(key.clone(), event, flags);
        let mut guard = self.inner.interests.lock();
        if guard.contains_key(&key) {
            return Err(AxError::AlreadyExists);
        }
        guard.insert(key.clone(), Arc::clone(&interest));
        drop(guard);
        trace!("Epoll add fd: {} interest {:?} ", fd, interest.event.events);
        self.check_and_register_waker(&interest);
        Ok(())
    }

    pub fn modify(&self, fd: i32, event: EpollEvent, flags: EpollFlags) -> AxResult<()> {
        let key = EntryKey::new(fd)?;
        let interest = EpollInterest::new_shared(key.clone(), event, flags);

        let mut guard = self.inner.interests.lock();
        let old = guard.get_mut(&key).ok_or(AxError::NotFound)?;

        // The ready queue stores a Weak pointer to the old interest. Its
        // queued state cannot be transferred to this replacement: doing so
        // leaves the new interest marked as queued even though no queue entry
        // can upgrade to it, and all later wakeups are then lost. The stale
        // weak entry is harmless; check/register the replacement below.
        *old = Arc::clone(&interest);
        drop(guard);
        trace!(
            "Epoll: modify fd={}, events={:?}",
            fd, interest.event.events
        );
        // reset waker
        self.check_and_register_waker(&interest);
        Ok(())
    }

    pub fn delete(&self, fd: i32) -> AxResult<()> {
        let key = EntryKey::new(fd)?;
        self.inner
            .interests
            .lock()
            .remove(&key)
            .ok_or(AxError::NotFound)?;
        trace!("Epoll: delete fd={fd}");
        Ok(())
    }

    pub fn has_ready_events(&self) -> bool {
        self.refresh_level_ready_interests();
        !self.inner.ready_queue.lock().is_empty()
    }

    /// Recovers level-triggered readiness even if an earlier registered waker
    /// became stale during `EPOLL_CTL_MOD` or was otherwise consumed before it
    /// could enqueue the current interest. Linux level-triggered epoll must
    /// continue reporting a file while its condition remains true; it cannot
    /// rely on receiving a second readiness edge.
    fn refresh_level_ready_interests(&self) {
        let interests = self
            .inner
            .interests
            .lock()
            .values()
            .cloned()
            .collect::<Vec<_>>();

        for interest in interests {
            if !interest.is_enabled() || !interest.is_level_triggered() {
                continue;
            }
            let Some(file) = interest.key.get_file() else {
                continue;
            };
            if !interest.ready_events(file.as_ref()).is_empty() && interest.try_mark_in_queue() {
                self.inner
                    .ready_queue
                    .lock()
                    .push_back(Arc::downgrade(&interest));
            }
        }
    }

    pub fn poll_events(&self, out: &mut [epoll_event]) -> AxResult<usize> {
        trace!("Epoll: poll_events called, out.len()={}", out.len());
        self.refresh_level_ready_interests();
        let mut count = 0;
        let mut requeue = Vec::new();
        loop {
            let weak_interest = {
                let mut queue = self.inner.ready_queue.lock();
                queue.pop_front()
            };

            let Some(weak_interest) = weak_interest else {
                break;
            };

            if count >= out.len() {
                self.inner.ready_queue.lock().push_front(weak_interest);
                break;
            }

            let Some(interest) = weak_interest.upgrade() else {
                continue; // interest already removed
            };

            let Some(file) = interest.key.get_file() else {
                // file already closed remove interests
                self.inner.interests.lock().remove(&interest.key);
                interest.mark_not_in_queue();
                continue;
            };

            trace!(
                "Epoll: consuming ready interest for fd={}, events={:?}",
                interest.key.fd, interest.event.events
            );

            match interest.consume(file.as_ref()) {
                ConsumeResult::EventAndKeep(event) => {
                    out[count] = epoll_event {
                        events: event.events.bits(),
                        data: event.user_data,
                    };
                    count += 1;
                    requeue.push(Arc::downgrade(&interest));
                }
                ConsumeResult::EventAndRemove(event) => {
                    out[count] = epoll_event {
                        events: event.events.bits(),
                        data: event.user_data,
                    };
                    count += 1;
                    interest.mark_not_in_queue();
                    self.register_waker_only(&interest);
                }
                ConsumeResult::NoEvent => {
                    interest.mark_not_in_queue();
                    self.register_waker_only(&interest);
                }
            }
        }

        if !requeue.is_empty() {
            self.inner.ready_queue.lock().extend(requeue);
        }

        if count == 0 {
            Err(AxError::WouldBlock)
        } else {
            Ok(count)
        }
    }
}

impl FileLike for Epoll {
    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[eventpoll]".into()
    }
}

impl Pollable for Epoll {
    fn poll(&self) -> IoEvents {
        if self.inner.ready_queue.lock().is_empty() {
            IoEvents::empty()
        } else {
            IoEvents::IN
        }
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.contains(IoEvents::IN) {
            self.inner.poll_ready.register(context.waker());
        }
    }
}
