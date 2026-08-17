//! Time management module.

use alloc::{borrow::ToOwned, sync::Arc, vec::Vec};
use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use axhal::time::{NANOS_PER_SEC, TimeValue, monotonic_time, monotonic_time_nanos, wall_time};
use axsync::spin::SpinNoIrq;
use axtask::{
    AxTaskRef, WeakAxTaskRef, current,
    future::{block_on, timeout_at},
};
use event_listener::{Event, listener};
use lazy_static::lazy_static;
use starry_signal::Signo;
use strum::FromRepr;

use crate::task::{AsThread, poll_timer};

fn time_value_from_nanos(nanos: usize) -> TimeValue {
    let secs = nanos as u64 / NANOS_PER_SEC;
    let nsecs = nanos as u64 - secs * NANOS_PER_SEC;
    TimeValue::new(secs, nsecs as u32)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TimerKey {
    deadline: Duration,
    seq: u64,
}

#[derive(Clone)]
struct Entry {
    task: WeakAxTaskRef,
    ty: ITimerType,
}

struct AlarmRuntime {
    timers: Vec<(TimerKey, Entry)>,
}

impl AlarmRuntime {
    const fn new() -> Self {
        Self { timers: Vec::new() }
    }

    fn insert(&mut self, deadline: Duration, task: WeakAxTaskRef, ty: ITimerType) -> TimerKey {
        let key = TimerKey {
            deadline,
            seq: NEXT_ALARM_SEQ.fetch_add(1, Ordering::Relaxed),
        };
        // The number of process interval timers is small. A linear queue avoids
        // relying on alloc::collections::BTreeMap on this LoongArch target; the
        // latter repeatedly ended up with a low root pointer under timer churn.
        self.timers.push((key, Entry { task, ty }));
        key
    }

    fn remove(&mut self, key: TimerKey) -> Option<Entry> {
        let index = self
            .timers
            .iter()
            .position(|(candidate, _)| *candidate == key)?;
        Some(self.timers.swap_remove(index).1)
    }

    fn first(&self) -> Option<(TimerKey, Entry)> {
        self.timers
            .iter()
            .min_by_key(|(key, _)| *key)
            .map(|(key, entry)| (*key, entry.clone()))
    }

    fn first_key(&self) -> Option<TimerKey> {
        self.timers.iter().map(|(key, _)| *key).min()
    }

    fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }
}

static NEXT_ALARM_SEQ: AtomicU64 = AtomicU64::new(0);

lazy_static! {
    static ref ALARM_RUNTIME: SpinNoIrq<AlarmRuntime> = SpinNoIrq::new(AlarmRuntime::new());
    static ref EVENT_NEW_TIMER: Event = Event::new();
}

/// The type of interval timer.
#[repr(i32)]
#[allow(non_camel_case_types)]
#[derive(Eq, PartialEq, Debug, Clone, Copy, FromRepr)]
pub enum ITimerType {
    /// 统计系统实际运行时间
    Real    = 0,
    /// 统计用户态运行时间
    Virtual = 1,
    /// 统计进程的所有用户态/内核态运行时间
    Prof    = 2,
}

impl ITimerType {
    /// Returns the signal number associated with this timer type.
    pub fn signo(&self) -> Signo {
        match self {
            ITimerType::Real => Signo::SIGALRM,
            ITimerType::Virtual => Signo::SIGVTALRM,
            ITimerType::Prof => Signo::SIGPROF,
        }
    }
}

#[derive(Default)]
struct ITimer {
    interval_ns: usize,
    remained_ns: usize,
    active_key: Option<TimerKey>,
}

impl ITimer {
    pub fn set(
        &mut self,
        ty: ITimerType,
        interval_ns: usize,
        remained_ns: usize,
    ) -> (TimeValue, TimeValue) {
        let old = (
            time_value_from_nanos(self.interval_ns),
            time_value_from_nanos(self.remained_ns),
        );
        self.cancel_timer();
        self.interval_ns = interval_ns;
        self.remained_ns = remained_ns;
        let owner = current().clone();
        self.arm_timer(&owner, ty);
        EVENT_NEW_TIMER.notify(1);
        old
    }

    pub fn update(&mut self, owner: &AxTaskRef, ty: ITimerType, delta: usize) -> bool {
        if self.remained_ns == 0 {
            return false;
        }
        if self.remained_ns > delta {
            self.remained_ns -= delta;
            false
        } else {
            self.remained_ns = self.interval_ns;
            self.arm_timer(owner, ty);
            true
        }
    }

    fn cancel_timer(&mut self) {
        if let Some(key) = self.active_key.take() {
            ALARM_RUNTIME.lock().remove(key);
        }
    }

    fn arm_timer(&mut self, owner: &AxTaskRef, ty: ITimerType) {
        self.cancel_timer();
        if self.remained_ns > 0 {
            let deadline = monotonic_time() + Duration::from_nanos(self.remained_ns as u64);
            self.active_key = Some(ALARM_RUNTIME.lock().insert(
                deadline,
                Arc::downgrade(owner),
                ty,
            ));
            EVENT_NEW_TIMER.notify(1);
        }
    }
}

/// Represents the state of the timer.
#[derive(Debug)]
pub enum TimerState {
    /// Fallback state.
    None,
    /// The timer is running in user space.
    User,
    /// The timer is running in kernel space.
    Kernel,
}

// TODO(mivik): preempting does not change the timer state currently
/// A manager for time-related operations.
pub struct TimeManager {
    utime_ns: usize,
    stime_ns: usize,
    last_wall_ns: usize,
    state: TimerState,
    itimers: [ITimer; 3],
}

impl Default for TimeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeManager {
    pub(crate) fn new() -> Self {
        Self {
            utime_ns: 0,
            stime_ns: 0,
            last_wall_ns: 0,
            state: TimerState::None,
            itimers: Default::default(),
        }
    }

    /// Returns the current user time and system time as a tuple of `TimeValue`.
    pub fn output(&self) -> (TimeValue, TimeValue) {
        let utime = time_value_from_nanos(self.utime_ns);
        let stime = time_value_from_nanos(self.stime_ns);
        (utime, stime)
    }

    /// Polls the time manager to update the timers and emit signals if
    /// necessary.
    pub fn poll(&mut self, owner: &AxTaskRef, emitter: impl Fn(Signo)) {
        let now_ns = monotonic_time_nanos() as usize;
        let delta = now_ns - self.last_wall_ns;
        match self.state {
            TimerState::User => {
                self.utime_ns += delta;
                self.update_itimer(owner, ITimerType::Virtual, delta, &emitter);
                self.update_itimer(owner, ITimerType::Prof, delta, &emitter);
            }
            TimerState::Kernel => {
                self.stime_ns += delta;
                self.update_itimer(owner, ITimerType::Prof, delta, &emitter);
            }
            TimerState::None => {}
        }
        self.update_itimer(owner, ITimerType::Real, delta, &emitter);
        self.last_wall_ns = now_ns;
    }

    /// Updates the timer state.
    pub fn set_state(&mut self, state: TimerState) {
        self.state = state;
    }

    /// Sets the interval timer of the specified type with the given interval
    /// and remaining time.
    pub fn set_itimer(
        &mut self,
        ty: ITimerType,
        interval_ns: usize,
        remained_ns: usize,
    ) -> (TimeValue, TimeValue) {
        self.itimers[ty as usize].set(ty, interval_ns, remained_ns)
    }

    /// Gets the current interval and remaining time.
    pub fn get_itimer(&self, ty: ITimerType) -> (TimeValue, TimeValue) {
        let itimer = &self.itimers[ty as usize];
        (
            time_value_from_nanos(itimer.interval_ns),
            time_value_from_nanos(itimer.remained_ns),
        )
    }

    /// Cancels all active interval timers owned by this thread.
    pub fn cancel_all(&mut self) {
        for itimer in &mut self.itimers {
            itimer.cancel_timer();
        }
    }

    fn update_itimer(
        &mut self,
        owner: &AxTaskRef,
        ty: ITimerType,
        delta: usize,
        emitter: impl Fn(Signo),
    ) {
        if self.itimers[ty as usize].update(owner, ty, delta) {
            emitter(ty.signo());
        }
    }

    fn entry_is_current(&self, key: TimerKey, entry: &Entry) -> bool {
        let timer = &self.itimers[entry.ty as usize];
        timer.remained_ns > 0 && timer.active_key == Some(key)
    }
}

impl Drop for TimeManager {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

fn entry_is_current_for_task(task: &AxTaskRef, key: TimerKey, entry: &Entry) -> bool {
    let Some(thr) = task.try_as_thread() else {
        return false;
    };
    let Some(time) = thr.time.try_lock() else {
        return true;
    };
    time.entry_is_current(key, entry)
}

async fn alarm_task() {
    loop {
        let mut guard = ALARM_RUNTIME.lock();
        let Some((key, entry)) = guard.first() else {
            drop(guard);
            listener!(EVENT_NEW_TIMER => listener);

            if !ALARM_RUNTIME.lock().is_empty() {
                continue;
            }
            listener.await;

            continue;
        };

        let Some(task) = entry.task.upgrade() else {
            guard.remove(key);
            continue;
        };
        if !entry_is_current_for_task(&task, key, &entry) {
            guard.remove(key);
            continue;
        }

        let now = monotonic_time();
        if key.deadline <= now {
            let Some(entry) = guard.remove(key) else {
                continue;
            };
            drop(guard);
            if let Some(task) = entry.task.upgrade()
                && entry_is_current_for_task(&task, key, &entry)
            {
                poll_timer(&task);
            }
        } else {
            let deadline = key.deadline;
            let timeout_deadline = wall_time()
                .checked_add(deadline.saturating_sub(now))
                .unwrap_or(TimeValue::MAX);
            drop(guard);
            listener!(EVENT_NEW_TIMER => listener);
            if ALARM_RUNTIME.lock().first_key().is_none_or(|it| it != key) {
                continue;
            }
            let _ = timeout_at(Some(timeout_deadline), listener).await;
        }
    }
}

/// Spawns the alarm task.
pub fn spawn_alarm_task() {
    info!("Initialize alarm...");
    axtask::spawn_raw(
        || block_on(alarm_task()),
        "alarm_task".to_owned(),
        axconfig::TASK_STACK_SIZE,
    );
}
