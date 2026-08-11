//! A blocking mutex implementation.

use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(target_arch = "loongarch64"))]
use core::sync::atomic::AtomicUsize;

use axtask::{current, yield_now};
#[cfg(not(target_arch = "loongarch64"))]
use axtask::future::block_on;
#[cfg(not(target_arch = "loongarch64"))]
use event_listener::{Event, listener};

/// A [`lock_api::RawMutex`] implementation.
///
/// When the mutex is locked, the current task will block and be put into the
/// wait queue. When the mutex is unlocked, one task waiting on the queue
/// will be woken up.
pub struct RawMutex {
    #[cfg(not(target_arch = "loongarch64"))]
    event: Event,
    owner_id: AtomicU64,
    #[cfg(not(target_arch = "loongarch64"))]
    waiters: AtomicUsize,
}

impl RawMutex {
    /// Creates a [`RawMutex`].
    #[inline(always)]
    pub const fn new() -> Self {
        Self {
            #[cfg(not(target_arch = "loongarch64"))]
            event: Event::new(),
            owner_id: AtomicU64::new(0),
            #[cfg(not(target_arch = "loongarch64"))]
            waiters: AtomicUsize::new(0),
        }
    }
}

impl Default for RawMutex {
    fn default() -> Self {
        Self::new()
    }
}

struct Spin(u32);

impl Spin {
    #[inline]
    fn spin(&mut self) -> bool {
        if self.0 >= 3 {
            return false;
        }
        self.0 += 1;
        for _ in 0..(1 << self.0) {
            core::hint::spin_loop();
        }
        true
    }
}

unsafe impl lock_api::RawMutex for RawMutex {
    type GuardMarker = lock_api::GuardSend;

    /// Initial value for an unlocked mutex.
    ///
    /// A “non-constant” const item is a legacy way to supply an initialized
    /// value to downstream static items. Can hopefully be replaced with
    /// `const fn new() -> Self` at some point.
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawMutex::new();

    #[inline(always)]
    #[track_caller]
    fn lock(&self) {
        let current_id = current().id().as_u64();
        let mut spin = Spin(0);
        let mut owner_id = self.owner_id.load(Ordering::Relaxed);
        #[cfg(target_arch = "loongarch64")]
        let mut waiting = false;

        loop {
            #[cfg(target_arch = "loongarch64")]
            if owner_id != 0
                && owner_id != current_id
                && axtask::task_by_id(owner_id)
                    .is_none_or(|owner| owner.state() == axtask::TaskState::Exited)
            {
                match self.owner_id.compare_exchange(
                    owner_id,
                    current_id,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        axtask::report_recovered_mutex(
                            self as *const Self as usize,
                            owner_id,
                            core::panic::Location::caller(),
                        );
                        break;
                    }
                    Err(x) => {
                        owner_id = x;
                        continue;
                    }
                }
            }

            assert_ne!(
                owner_id,
                current_id,
                "{} tried to acquire mutex {:p} it already owns.",
                current().id_name(),
                self
            );

            if owner_id == 0 {
                match self.owner_id.compare_exchange_weak(
                    owner_id,
                    current_id,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(x) => owner_id = x,
                }
                continue;
            }

            if spin.spin() {
                owner_id = self.owner_id.load(Ordering::Relaxed);
                continue;
            }

            // On the LS2K1000LA, the event-listener slow path has repeatedly
            // observed Event::inner as the invalid pointer 0x1 under the
            // cagent local-TCP workload.  Keep the mutex ownership protocol
            // unchanged, but avoid that heap-backed listener path on
            // LoongArch while its lifetime corruption is isolated.  Yielding
            // still lets the owner run and release the mutex, and leaves all
            // other architectures on the normal blocking implementation.
            #[cfg(target_arch = "loongarch64")]
            {
                if !waiting {
                    let caller = core::panic::Location::caller() as *const _ as usize;
                    current().begin_mutex_wait(
                        self as *const Self as usize,
                        owner_id,
                        caller,
                    );
                    waiting = true;
                }
                yield_now();
                owner_id = self.owner_id.load(Ordering::Acquire);
                current().update_mutex_wait_owner(owner_id);
                continue;
            }

            #[cfg(not(target_arch = "loongarch64"))]
            {
                listener!(self.event => listener);

                self.waiters.fetch_add(1, Ordering::SeqCst);
                owner_id = self.owner_id.load(Ordering::SeqCst);
                if owner_id == 0 {
                    self.waiters.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }

                block_on(listener);
                self.waiters.fetch_sub(1, Ordering::SeqCst);
                owner_id = self.owner_id.load(Ordering::Acquire);
            }
        }

        #[cfg(target_arch = "loongarch64")]
        if waiting {
            current().end_mutex_wait();
        }
    }

    #[inline(always)]
    fn try_lock(&self) -> bool {
        let current_id = current().id().as_u64();
        // The reason for using a strong compare_exchange is explained here:
        // https://github.com/Amanieu/parking_lot/pull/207#issuecomment-575869107
        self.owner_id
            .compare_exchange(0, current_id, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline(always)]
    unsafe fn unlock(&self) {
        let owner_id = self.owner_id.swap(0, Ordering::SeqCst);
        assert_eq!(
            owner_id,
            current().id().as_u64(),
            "{} tried to release mutex it doesn't own",
            current().id_name()
        );
        #[cfg(not(target_arch = "loongarch64"))]
        {
            if self.waiters.load(Ordering::SeqCst) != 0 {
                self.event.notify(1);
            }
        }
    }

    #[inline(always)]
    fn is_locked(&self) -> bool {
        self.owner_id.load(Ordering::Relaxed) != 0
    }
}

/// An alias of [`lock_api::Mutex`].
pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;
/// An alias of [`lock_api::MutexGuard`].
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutex, T>;

/// Initializes a cooperative mutex directly in its final unpublished storage.
///
/// # Safety
///
/// `dst` must point to valid, aligned, uninitialized storage for one
/// [`Mutex<T>`]. The object must remain private until initialization finishes.
#[inline]
pub unsafe fn write_unpublished_in_place<T>(dst: *mut Mutex<T>, value: T) {
    unsafe { Mutex::write_unpublished_in_place(dst, value) }
}

/// Reinitializes the raw state of a newly constructed, unpublished mutex.
///
/// This is a narrow workaround for the pinned LoongArch toolchain corrupting
/// atomic fields while moving a large aggregate into its final allocation.
/// The protected value is preserved. On LoongArch the raw state consists only
/// of the owner atomic: that architecture already uses the cooperative-yield
/// slow path, so keeping an unused heap-backed `Event` would add movable and
/// droppable state without participating in lock or unlock.
///
/// # Safety
///
/// The mutex must be freshly constructed, exclusively borrowed, and not yet
/// visible to another task. It must never have had a waiter or a live guard.
#[inline]
pub unsafe fn reinitialize_unpublished<T>(mutex: &mut Mutex<T>) -> u64 {
    let raw = unsafe { mutex.raw_mut() };
    let previous_owner = raw.owner_id.load(Ordering::Relaxed);
    unsafe { core::ptr::write(raw, RawMutex::new()) };
    previous_owner
}

#[cfg(test)]
mod tests {
    use crate::Mutex;
    use axtask as thread;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn may_interrupt() {
        // simulate interrupts
        if fastrand::u8(0..3) == 0 {
            thread::yield_now();
        }
    }

    #[test]
    fn lots_and_lots() {
        INIT.call_once(thread::init_scheduler);

        const NUM_TASKS: u32 = 10;
        const NUM_ITERS: u32 = 10_000;
        static M: Mutex<u32> = Mutex::new(0);

        fn inc(delta: u32) {
            for _ in 0..NUM_ITERS {
                let mut val = M.lock();
                *val += delta;
                may_interrupt();
                drop(val);
                may_interrupt();
            }
        }

        for _ in 0..NUM_TASKS {
            thread::spawn(|| inc(1));
            thread::spawn(|| inc(2));
        }

        println!("spawn OK");
        loop {
            let val = M.lock();
            if *val == NUM_ITERS * NUM_TASKS * 3 {
                break;
            }
            may_interrupt();
            drop(val);
            may_interrupt();
        }

        assert_eq!(*M.lock(), NUM_ITERS * NUM_TASKS * 3);
        println!("Mutex test OK");
    }
}
