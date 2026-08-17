use alloc::collections::BTreeMap;
use core::{
    fmt,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};

use axerrno::AxError;
use axhal::time::{TimeValue, wall_time};
use futures_util::{FutureExt, select_biased};
use kspin::SpinNoIrq;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TimerKey {
    deadline: TimeValue,
    key: u64,
}

struct TimerRuntime {
    key: u64,
    wheel: BTreeMap<TimerKey, Waker>,
}

impl TimerRuntime {
    const fn new() -> Self {
        TimerRuntime {
            key: 0,
            wheel: BTreeMap::new(),
        }
    }

    fn add(&mut self, deadline: TimeValue) -> Option<TimerKey> {
        let now = wall_time();
        if deadline <= now {
            return None;
        }

        let key = TimerKey {
            deadline,
            key: self.key,
        };
        self.wheel.insert(key, Waker::noop().clone());
        self.key += 1;

        Some(key)
    }

    fn poll(&mut self, key: &TimerKey, cx: &mut Context<'_>) -> Poll<()> {
        if let Some(w) = self.wheel.get_mut(key) {
            *w = cx.waker().clone();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }

    fn cancel(&mut self, key: &TimerKey) {
        self.wheel.remove(key);
    }

    fn take_expired(&mut self) -> BTreeMap<TimerKey, Waker> {
        if self.wheel.is_empty() {
            return BTreeMap::new();
        }

        let now = wall_time();

        let pending = self.wheel.split_off(&TimerKey {
            deadline: now,
            key: u64::MAX,
        });

        core::mem::replace(&mut self.wheel, pending)
    }
}

// Futures may resume on a different CPU after they block. Keeping their timer
// entries in a per-CPU wheel therefore lets a migrated future poll or cancel
// the wrong wheel, which can turn a real deadline into an immediate timeout
// and leave the original entry behind. A shared wheel keeps the TimerKey valid
// for the whole future lifetime regardless of scheduler migration.
static TIMER_RUNTIME: SpinNoIrq<TimerRuntime> = SpinNoIrq::new(TimerRuntime::new());

#[allow(dead_code)]
pub(crate) fn check_timer_events() {
    // Wakers are arbitrary callbacks and may acquire epoll, socket, or task
    // locks.  Never invoke them while holding the global timer-wheel lock:
    // another CPU can be interrupted while holding one of those locks and
    // then wait for this timer lock, forming a cross-CPU ABBA deadlock.
    let expired = TIMER_RUNTIME.lock().take_expired();
    for (_, waker) in expired {
        waker.wake();
    }
}

fn with_current<R>(f: impl FnOnce(&mut TimerRuntime) -> R) -> R {
    f(&mut TIMER_RUNTIME.lock())
}

/// Future returned by `sleep` and `sleep_until`.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct TimerFuture(TimerKey);

impl Future for TimerFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        with_current(|r| r.poll(&self.0, cx))
    }
}

impl Drop for TimerFuture {
    fn drop(&mut self) {
        with_current(|r| r.cancel(&self.0));
    }
}

/// Waits until `duration` has elapsed.
pub async fn sleep(duration: Duration) {
    sleep_until(wall_time() + duration).await
}

/// Waits until `deadline` is reached.
pub async fn sleep_until(deadline: TimeValue) {
    let key = with_current(|r| r.add(deadline));
    if let Some(key) = key {
        TimerFuture(key).await;
    }
}

/// Error returned by [`timeout`] and [`timeout_at`].
#[derive(Debug, PartialEq, Eq)]
pub struct Elapsed(());

impl fmt::Display for Elapsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "deadline elapsed")
    }
}

impl core::error::Error for Elapsed {}

impl From<Elapsed> for AxError {
    fn from(_: Elapsed) -> Self {
        AxError::TimedOut
    }
}

/// Requires a `Future` to complete before the specified duration has elapsed.
pub async fn timeout<F: IntoFuture>(
    duration: Option<Duration>,
    f: F,
) -> Result<F::Output, Elapsed> {
    timeout_at(
        duration.and_then(|x| x.checked_add(axhal::time::wall_time())),
        f,
    )
    .await
}

/// Requires a `Future` to complete before the specified deadline.
pub async fn timeout_at<F: IntoFuture>(
    deadline: Option<TimeValue>,
    f: F,
) -> Result<F::Output, Elapsed> {
    if let Some(deadline) = deadline {
        select_biased! {
            res = f.into_future().fuse() => Ok(res),
            _ = sleep_until(deadline).fuse() => Err(Elapsed(())),
        }
    } else {
        Ok(f.await)
    }
}
