//! A library for polling I/O events and waking up tasks.

#![no_std]
#![deny(missing_docs)]

extern crate alloc;

use alloc::{boxed::Box, sync::Arc, task::Wake};
use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Waker},
};

use bitflags::bitflags;
use kernel_guard::NoPreemptIrqSave;
use linux_raw_sys::general::*;
use spin::{Lazy, Mutex};

bitflags! {
    /// I/O events.
    #[derive(Debug, Clone, Copy)]
    pub struct IoEvents: u32 {
        /// Available for read
        const IN     = POLLIN;
        /// Urgent data for read
        const PRI    = POLLPRI;
        /// Available for write
        const OUT    = POLLOUT;

        /// Error condition
        const ERR    = POLLERR;
        /// Hang up
        const HUP    = POLLHUP;
        /// Invalid request
        const NVAL   = POLLNVAL;

        /// Equivalent to [`IN`](Self::IN)
        const RDNORM = POLLRDNORM;
        /// Priority band data can be read
        const RDBAND = POLLRDBAND;
        /// Equivalent to [`OUT`](Self::OUT)
        const WRNORM = POLLWRNORM;
        /// Priority data can be written
        const WRBAND = POLLWRBAND;

        /// Message
        const MSG    = POLLMSG;
        /// Remove
        const REMOVE = POLLREMOVE;
        /// Stream socket peer closed connection, or shut down writing half of connection.
        const RDHUP  = POLLRDHUP;

        /// Events that are always polled even without specifying them.
        const ALWAYS_POLL = Self::ERR.bits() | Self::HUP.bits();
    }
}

/// Trait for types that can be polled for I/O events.
pub trait Pollable {
    /// Polls for I/O events.
    fn poll(&self) -> IoEvents;

    /// Registers wakers for I/O events.
    fn register(&self, context: &mut Context<'_>, events: IoEvents);
}

const POLL_SET_CAPACITY: usize = 64;

struct Inner {
    entries: Box<[MaybeUninit<Waker>]>,
    cursor: usize,
}

impl Inner {
    fn new() -> Self {
        Self {
            entries: Box::new_uninit_slice(POLL_SET_CAPACITY),
            cursor: 0,
        }
    }

    fn len(&self) -> usize {
        self.cursor.min(POLL_SET_CAPACITY)
    }

    fn is_empty(&self) -> bool {
        self.cursor == 0
    }

    fn register(&mut self, waker: &Waker) -> Option<Waker> {
        // Re-registering the same logical waiter is idempotent.  Keeping
        // duplicate clones wastes the bounded table and amplifies a single
        // source transition into many wakeups.
        if self
            .entries
            .iter()
            .take(self.len())
            .any(|entry| unsafe { entry.assume_init_ref() }.will_wake(waker))
        {
            return None;
        }

        let slot = self.cursor % POLL_SET_CAPACITY;
        let evicted = if self.cursor >= POLL_SET_CAPACITY {
            let old = unsafe { self.entries[slot].assume_init_read() };
            self.cursor = ((slot + 1) % POLL_SET_CAPACITY) + POLL_SET_CAPACITY;
            (!old.will_wake(waker)).then_some(old)
        } else {
            self.cursor += 1;
            None
        };
        self.entries[slot].write(waker.clone());
        evicted
    }

    fn take_wakers(&mut self) -> ([MaybeUninit<Waker>; POLL_SET_CAPACITY], usize) {
        let len = self.len();
        let mut wakers = [const { MaybeUninit::uninit() }; POLL_SET_CAPACITY];
        for (dst, src) in wakers.iter_mut().zip(self.entries.iter()).take(len) {
            dst.write(unsafe { src.assume_init_read() });
        }
        self.cursor = 0;
        (wakers, len)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        for i in 0..self.len() {
            unsafe { self.entries[i].assume_init_read() }.wake();
        }
    }
}

/// A data structure for waking up tasks that are waiting for I/O events.
pub struct PollSet {
    inner: Lazy<Mutex<Inner>>,
    has_waiters: AtomicBool,
}

impl Default for PollSet {
    fn default() -> Self {
        Self::new()
    }
}

impl PollSet {
    /// Creates a new empty [`PollSet`].
    pub const fn new() -> Self {
        Self {
            inner: Lazy::new(|| Mutex::new(Inner::new())),
            has_waiters: AtomicBool::new(false),
        }
    }

    /// Creates a shared poll set directly in its final allocation.
    pub fn new_shared() -> Arc<Self> {
        let mut storage = Arc::<Self>::new_uninit();
        let dst = Arc::get_mut(&mut storage)
            .expect("new poll-set allocation must be unique")
            .as_mut_ptr();
        unsafe {
            Self::write_unpublished_in_place(dst);
            storage.assume_init()
        }
    }

    /// Initializes a poll set directly in its final unpublished storage.
    ///
    /// # Safety
    ///
    /// `dst` must be valid, aligned, uninitialized storage for one `PollSet`
    /// and must not be published until initialization completes.
    pub unsafe fn write_unpublished_in_place(dst: *mut Self) {
        unsafe {
            core::ptr::addr_of_mut!((*dst).inner).write(Lazy::new(|| Mutex::new(Inner::new())));
            core::ptr::addr_of_mut!((*dst).has_waiters).write(AtomicBool::new(false));
        }
    }

    /// Registers a waker.
    pub fn register(&self, waker: &Waker) {
        let evicted = {
            // Poll sources can register from task context while another CPU
            // wakes the same set from a timer or device interrupt. Keep the
            // lock owner runnable and prevent same-CPU interrupt re-entry.
            let _irq_guard = NoPreemptIrqSave::new();
            let mut inner = self.inner.lock();
            self.has_waiters.store(true, Ordering::Release);
            inner.register(waker)
        };

        // Wakers are arbitrary callbacks and may re-enter this PollSet. Never
        // invoke one while the set lock or the IRQ/preemption guard is held.
        if let Some(waker) = evicted {
            waker.wake();
        }
    }

    /// Wakes up all registered wakers.
    pub fn wake(&self) -> usize {
        if !self.has_waiters.load(Ordering::Acquire) {
            return 0;
        }

        let (wakers, len) = {
            // A wake may originate in interrupt context. The plain spin mutex
            // still provides host-side synchronization, while this guard
            // supplies the kernel IRQ/preemption semantics it lacks.
            let _irq_guard = NoPreemptIrqSave::new();
            let mut guard = self.inner.lock();
            if guard.is_empty() {
                self.has_waiters.store(false, Ordering::Release);
                return 0;
            }
            let wakers = guard.take_wakers();
            self.has_waiters.store(false, Ordering::Release);
            wakers
        };
        for waker in wakers.iter().take(len) {
            unsafe { waker.assume_init_read() }.wake();
        }
        len
    }
}

impl Drop for PollSet {
    fn drop(&mut self) {
        // Ensure all entries are dropped
        self.wake();
    }
}

impl Wake for PollSet {
    fn wake(self: Arc<Self>) {
        self.as_ref().wake();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.as_ref().wake();
    }
}
