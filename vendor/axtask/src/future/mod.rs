//! Future support.

use alloc::{sync::Arc, task::Wake};
#[cfg(feature = "profile")]
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{
    fmt,
    future::poll_fn,
    pin::pin,
    task::{Context, Poll, Waker},
};

use axerrno::AxError;
use kernel_guard::NoPreemptIrqSave;
use kspin::SpinNoIrq;

use crate::{AxTaskRef, WeakAxTaskRef, current, current_run_queue, select_run_queue};

mod poll;
pub use poll::*;

mod time;
pub use time::*;

#[cfg(feature = "profile")]
type ProfileBeginHook = fn() -> u64;
#[cfg(feature = "profile")]
type ProfileEndHook = fn(u64);

#[cfg(feature = "profile")]
static PROFILE_BEGIN_HOOK: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "profile")]
static PROFILE_END_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Installs callbacks around every scheduling interval in which a task is
/// genuinely blocked off-CPU.
#[cfg(feature = "profile")]
pub fn register_block_profile_hooks(begin: ProfileBeginHook, end: ProfileEndHook) {
    PROFILE_BEGIN_HOOK.store(begin as usize, Ordering::Release);
    PROFILE_END_HOOK.store(end as usize, Ordering::Release);
}

#[cfg(feature = "profile")]
#[inline]
fn profile_block_begin() -> u64 {
    let hook = PROFILE_BEGIN_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return 0;
    }
    // SAFETY: `register_block_profile_hooks` stores this exact function type.
    let hook: ProfileBeginHook = unsafe { core::mem::transmute(hook) };
    hook()
}

#[cfg(feature = "profile")]
#[inline]
fn profile_block_end(token: u64) {
    if token == 0 {
        return;
    }
    let hook = PROFILE_END_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return;
    }
    // SAFETY: `register_block_profile_hooks` stores this exact function type.
    let hook: ProfileEndHook = unsafe { core::mem::transmute(hook) };
    hook(token);
}

struct AxWaker {
    task: WeakAxTaskRef,
    woke: SpinNoIrq<bool>,
}

impl AxWaker {
    fn new(task: &AxTaskRef) -> Arc<Self> {
        Arc::new(AxWaker {
            task: Arc::downgrade(task),
            woke: SpinNoIrq::new(false),
        })
    }
}

impl Wake for AxWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(task) = self.task.upgrade() {
            *self.woke.lock() = true;
            select_run_queue::<NoPreemptIrqSave>(&task).unblock_task(task, false);
        }
    }
}

/// Blocks the current task until the given future is resolved.
///
/// Note that this doesn't handle interruption and is not recommended for direct
/// use in most cases.
pub fn block_on<F: IntoFuture>(f: F) -> F::Output {
    let mut fut = pin!(f.into_future());

    let curr = current();
    // It's necessary to keep a strong reference to the current task
    // to prevent it from being dropped while blocking.
    let task = curr.clone();

    let axwaker = AxWaker::new(&task);
    let waker = Waker::from(axwaker.clone());
    let mut cx = Context::from_waker(&waker);

    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Pending => {
                let mut rq = current_run_queue::<NoPreemptIrqSave>();
                let mut woke = axwaker.woke.lock();
                if !*woke {
                    #[cfg(feature = "profile")]
                    let profile_token = profile_block_begin();
                    rq.blocked_resched(woke);
                    #[cfg(feature = "profile")]
                    profile_block_end(profile_token);
                } else {
                    // The future woke itself while it was being polled. Clear
                    // the edge-triggered flag and poll it again immediately;
                    // yielding here can leave `woke` permanently set and turn
                    // a single notification into an endless yield loop.
                    *woke = false;
                    drop(woke);
                }
            }
            Poll::Ready(output) => break output,
        }
    }
}

/// Error returned by [`interruptible`].
#[derive(Debug, PartialEq, Eq)]
pub struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "interrupted")
    }
}

impl core::error::Error for Interrupted {}

impl From<Interrupted> for AxError {
    fn from(_: Interrupted) -> Self {
        AxError::Interrupted
    }
}

/// Makes a future interruptible.
pub async fn interruptible<F: IntoFuture>(f: F) -> Result<F::Output, Interrupted> {
    let mut f = pin!(f.into_future());
    let curr = current();
    poll_fn(|cx| {
        if curr.poll_interrupt(cx).is_ready() {
            return Poll::Ready(Err(Interrupted));
        }
        f.as_mut().poll(cx).map(Ok)
    })
    .await
}
