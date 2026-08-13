//! Futex implementation.

use alloc::{
    collections::{btree_map::BTreeMap, vec_deque::VecDeque},
    sync::{Arc, Weak},
};
use core::{
    future::poll_fn,
    ops::Deref,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
    time::Duration,
};

use axerrno::{AxError, AxResult};
use axpoll::PollSet;
use axsync::Mutex;
use axtask::{
    current,
    future::{block_on, interruptible, timeout},
};
use hashbrown::HashMap;
use kspin::SpinNoIrq;
use memory_addr::VirtAddr;

use crate::{
    mm::{AddrSpace, Backend, SharedPages},
    task::AsThread,
};

/// Wait queue used by futex.
#[derive(Default)]
pub struct WaitQueue {
    queue: SpinNoIrq<VecDeque<(Arc<AtomicBool>, u32, Arc<PollSet>)>>,
}
impl WaitQueue {
    /// Creates a new `WaitQueue`.
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn register_waiter(
        &self,
        bitset: u32,
        notified: Arc<AtomicBool>,
        poll: Arc<PollSet>,
        condition: impl FnOnce() -> bool,
    ) -> bool {
        let mut queue = self.queue.lock();

        if !condition() {
            return false;
        }

        queue.push_back((
            notified, 
            bitset, 
            poll
        ));
        true
    }

    pub fn unregister_waiter(
        &self,
        notified: &Arc<AtomicBool>,
    ) {
        self.queue.lock().retain(
            |(waiter, ..)| {
                !Arc::ptr_eq(waiter, notified)
            },
        );
    }

    /// Waits if the given condition is met.
    ///
    /// Returns `false` if the condition is not met and no actual waiting
    /// occurs.
    pub fn wait_if(
        &self,
        bitset: u32,
        timeout_dur: Option<Duration>,
        condition: impl FnOnce() -> bool,
    ) -> AxResult<bool> {
        let notified = Arc::new(AtomicBool::new(false));
        let poll = Arc::new(PollSet::new());

        {
            if !self.register_waiter(
                bitset,
                notified.clone(),
                poll.clone(),
                condition,
            ) {
                return Ok(false);
            }
        }

        let result = block_on(interruptible(timeout(
            timeout_dur,
            poll_fn(|cx| {
                if notified.load(Ordering::Acquire) {
                    return Poll::Ready(());
                }
                poll.register(cx.waker());
                if notified.load(Ordering::Acquire) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }),
        )));

        match result {
            Ok(Ok(())) => Ok(true),
            Ok(Err(_)) => {
                self.unregister_waiter(&notified);
                Err(AxError::TimedOut)
            }
            Err(_) => {
                self.unregister_waiter(& notified);
                Err(AxError::Interrupted)
            }
        }
    }

    /// Wakes up at most `count` tasks whose bitset intersects with the given
    /// bitmask.
    pub fn wake(&self, count: usize, mask: u32) -> usize {
        let mut woke = 0;
        self.queue.lock().retain(|(waiter, bitset, poll)| {
            if woke >= count || (bitset & mask) == 0 {
                true
            } else {
                waiter.store(true, Ordering::Release);
                poll.wake();
                woke += 1;
                false
            }
        });
        woke
    }

    /// Checks if the wait queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }

    /// Wakes waiters and moves the remainder to `target` as one atomic queue
    /// operation. A gap between removing from the source and inserting into
    /// the target loses a concurrent wake on the target futex and can leave a
    /// pthread condition-variable waiter asleep forever.
    pub fn wake_requeue(
        &self,
        wake_count: usize,
        requeue_count: usize,
        target: &WaitQueue,
    ) -> usize {
        if core::ptr::eq(self, target) {
            return self.wake(wake_count, u32::MAX);
        }

        fn apply(
            source: &mut VecDeque<(Arc<AtomicBool>, u32, Arc<PollSet>)>,
            target: &mut VecDeque<(Arc<AtomicBool>, u32, Arc<PollSet>)>,
            wake_count: usize,
            requeue_count: usize,
        ) -> usize {
            let mut woke = 0;
            source.retain(|(waiter, _bitset, poll)| {
                if woke == wake_count {
                    true
                } else {
                    waiter.store(true, Ordering::Release);
                    poll.wake();
                    woke += 1;
                    false
                }
            });

            let moved = requeue_count.min(source.len());
            target.extend(source.drain(..moved));
            woke + moved
        }

        // A futex can be requeued in both directions. Use a stable address
        // order so two concurrent reverse requeues cannot deadlock.
        let source_addr = core::ptr::from_ref(&self.queue).addr();
        let target_addr = core::ptr::from_ref(&target.queue).addr();
        if source_addr < target_addr {
            let mut source = self.queue.lock();
            let mut target = target.queue.lock();
            apply(&mut source, &mut target, wake_count, requeue_count)
        } else {
            let mut target = target.queue.lock();
            let mut source = self.queue.lock();
            apply(&mut source, &mut target, wake_count, requeue_count)
        }
    }
}

/// A key that uniquely identifies a futex in the system.
pub enum FutexKey {
    /// A futex that is private to the current process.
    Private {
        /// The memory address of the futex.
        address: usize,
    },

    /// A futex in a shared memory region.
    Shared {
        /// The offset of the futex within the shared memory region.
        offset: usize,
        /// The shared memory region.
        region: Result<Weak<SharedPages>, Weak<()>>,
    },
}

impl FutexKey {
    /// Creates a new `FutexKey`.
    pub fn new(aspace: &AddrSpace, address: usize) -> Self {
        if let Some(area) = aspace.find_area(VirtAddr::from_usize(address)) {
            match area.backend() {
                Backend::Shared(backend) => {
                    return Self::Shared {
                        offset: address - area.start().as_usize(),
                        region: Ok(Arc::downgrade(backend.pages())),
                    };
                }
                Backend::File(file) => {
                    return Self::Shared {
                        offset: address - area.start().as_usize(),
                        region: Err(file.futex_handle()),
                    };
                }
                _ => {}
            }
        }
        Self::Private { address }
    }

    /// Shortcut to create a `FutexKey` for the current task's address space.
    pub fn new_current(address: usize) -> Self {
        Self::new(&current().as_thread().proc_data.aspace().lock(), address)
    }

    fn as_usize(&self) -> usize {
        match self {
            FutexKey::Private { address } => *address,
            FutexKey::Shared { offset, .. } => *offset,
        }
    }
}

/// The futex entry structure
pub struct FutexEntry {
    /// The wait queue associated with this futex.
    pub wq: WaitQueue,

    /// Used by robust list, indicates if the owner of this futex is dead.
    pub owner_dead: AtomicBool,
}

impl FutexEntry {
    fn new() -> Self {
        Self {
            wq: WaitQueue::new(),
            owner_dead: AtomicBool::new(false),
        }
    }
}

/// A table mapping memory addresses to futex wait queues.
pub struct FutexTable(Mutex<HashMap<usize, Arc<FutexEntry>>>);

impl FutexTable {
    /// Creates a new `FutexTable`.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    /// Checks if the futex table is empty.
    pub fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }

    /// Gets the wait queue associated with the given address.
    pub fn get(&self, key: &FutexKey) -> Option<FutexGuard<'_>> {
        let key = key.as_usize();
        let entry = self.0.lock().get(&key).cloned()?;
        Some(FutexGuard {
            table: self,
            key,
            inner: entry,
        })
    }

    /// Gets the wait queue associated with the given address, or inserts a a
    /// new one if it doesn't exist.
    pub fn get_or_insert(&self, key: &FutexKey) -> FutexGuard<'_> {
        let key = key.as_usize();
        let mut table = self.0.lock();
        let entry = table
            .entry(key)
            .or_insert_with(|| Arc::new(FutexEntry::new()));
        FutexGuard {
            table: self,
            key,
            inner: entry.clone(),
        }
    }
}

#[doc(hidden)]
pub struct FutexGuard<'a> {
    table: &'a FutexTable,
    key: usize,
    inner: Arc<FutexEntry>,
}

impl Deref for FutexGuard<'_> {
    type Target = Arc<FutexEntry>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Drop for FutexGuard<'_> {
    fn drop(&mut self) {
        // Most guards still have another user or a queued waiter. Avoid
        // contending on the table lock unless this entry may be unused.
        if Arc::strong_count(&self.inner) > 2 || !self.inner.wq.is_empty() {
            return;
        }

        let mut table = self.table.0.lock();
        let can_remove = table.get(&self.key).is_some_and(|entry| {
            Arc::ptr_eq(entry, &self.inner) && Arc::strong_count(entry) <= 2 && entry.wq.is_empty()
        });
        if can_remove {
            table.remove(&self.key);
        }
    }
}

struct FutexTables {
    map: BTreeMap<usize, Arc<FutexTable>>,
    operations: usize,
}
impl FutexTables {
    const fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            operations: 0,
        }
    }

    fn get_or_insert(&mut self, key: usize) -> Arc<FutexTable> {
        self.operations += 1;
        if self.operations == 100 {
            self.operations = 0;
            self.map
                .retain(|_, table| Arc::strong_count(table) > 1 || !table.is_empty());
        }
        self.map
            .entry(key)
            .or_insert_with(|| Arc::new(FutexTable::new()))
            .clone()
    }
}

static SHARED_FUTEX_TABLES: Mutex<FutexTables> = Mutex::new(FutexTables::new());

/// Returns the futex table for the given key.
pub fn futex_table_for(key: &FutexKey) -> Arc<FutexTable> {
    match key {
        FutexKey::Private { .. } => current().as_thread().proc_data.futex_table.clone(),
        FutexKey::Shared { region, .. } => {
            let ptr = match region {
                Ok(pages) => Weak::as_ptr(pages) as usize,
                Err(key) => Weak::as_ptr(key) as usize,
            };
            SHARED_FUTEX_TABLES.lock().get_or_insert(ptr)
        }
    }
}
