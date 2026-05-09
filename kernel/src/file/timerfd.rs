use alloc::{borrow::Cow, sync::Arc};
use core::{
    sync::atomic::{AtomicBool, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult};
use axhal::time::TimeValue;
use axpoll::{IoEvents, PollSet, Pollable};
use axtask::future::{block_on, poll_io, timeout_at};
use event_listener::{Event, listener};
use linux_raw_sys::general::{CLOCK_REALTIME, CLOCK_REALTIME_ALARM, CLOCK_TAI};
use spin::Mutex;

use crate::file::{FileLike, IoDst};

#[derive(Debug, Clone, Copy, Default)]
pub struct KernelTimerSpec {
    pub interval: TimeValue,
    pub value: TimeValue,
}

impl KernelTimerSpec {
    pub fn disarmed(&self) -> bool {
        self.value.is_zero()
    }
}

#[derive(Debug, Clone, Copy)]
struct TimerFdState {
    interval: TimeValue,
    deadline: Option<TimeValue>,
    expirations: u64,
    generation: u64,
}

pub struct TimerFd {
    clock_id: u32,
    non_blocking: AtomicBool,
    worker_running: AtomicBool,
    state: Mutex<TimerFdState>,
    poll_rx: PollSet,
    update_event: Event,
}

impl TimerFd {
    pub fn new(clock_id: u32) -> Arc<Self> {
        Arc::new(Self {
            clock_id,
            non_blocking: AtomicBool::new(false),
            worker_running: AtomicBool::new(false),
            state: Mutex::new(TimerFdState {
                interval: TimeValue::ZERO,
                deadline: None,
                expirations: 0,
                generation: 0,
            }),
            poll_rx: PollSet::new(),
            update_event: Event::new(),
        })
    }

    pub fn gettime(&self) -> KernelTimerSpec {
        let state = self.state.lock();
        KernelTimerSpec {
            interval: state.interval,
            value: state
                .deadline
                .map(|deadline| deadline.saturating_sub(timer_clock_now(self.clock_id)))
                .unwrap_or(TimeValue::ZERO),
        }
    }

    pub fn settime(self: &Arc<Self>, spec: KernelTimerSpec, abstime: bool) -> KernelTimerSpec {
        let now = timer_clock_now(self.clock_id);
        let mut state = self.state.lock();
        let old = KernelTimerSpec {
            interval: state.interval,
            value: state
                .deadline
                .map(|deadline| deadline.saturating_sub(now))
                .unwrap_or(TimeValue::ZERO),
        };

        state.generation = state.generation.wrapping_add(1);
        state.interval = spec.interval;
        state.deadline = if spec.disarmed() {
            None
        } else if abstime {
            Some(adjust_timerfd_absolute_deadline(now, spec.value))
        } else {
            Some(now.checked_add(spec.value).unwrap_or(TimeValue::MAX))
        };
        drop(state);

        self.update_event.notify(1);
        self.arm();
        old
    }

    fn arm(self: &Arc<Self>) {
        if self.armed_snapshot().is_none() {
            return;
        };
        if self
            .worker_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let timerfd = self.clone();
        axtask::spawn_raw(
            move || timerfd_task(timerfd),
            "timerfd".into(),
            axconfig::TASK_STACK_SIZE,
        );
    }

    fn armed_snapshot(&self) -> Option<(TimeValue, u64)> {
        let state = self.state.lock();
        state.deadline.map(|deadline| (deadline, state.generation))
    }

    fn fire(&self, generation: u64) -> bool {
        let now = timer_clock_now(self.clock_id);
        let mut state = self.state.lock();
        if state.generation != generation {
            return false;
        }
        let Some(deadline) = state.deadline else {
            return false;
        };
        if now < deadline {
            return true;
        }

        let missed = if state.interval.is_zero() {
            1
        } else {
            1 + now
                .saturating_sub(deadline)
                .as_nanos()
                .checked_div(state.interval.as_nanos())
                .unwrap_or(0)
        };
        state.expirations = state
            .expirations
            .saturating_add(missed.min(u64::MAX as u128) as u64);
        if state.interval.is_zero() {
            state.deadline = None;
        } else {
            let advance = duration_mul(state.interval, missed);
            state.deadline = Some(deadline.checked_add(advance).unwrap_or(TimeValue::MAX));
        }
        drop(state);
        self.poll_rx.wake();
        true
    }
}

impl FileLike for TimerFd {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        if dst.remaining_mut() < size_of::<u64>() {
            return Err(AxError::InvalidInput);
        }

        block_on(poll_io(self, IoEvents::IN, self.nonblocking(), || {
            let mut state = self.state.lock();
            if state.expirations == 0 {
                return Err(AxError::WouldBlock);
            }
            let expirations = state.expirations;
            state.expirations = 0;
            drop(state);
            dst.write(&expirations.to_ne_bytes())?;
            Ok(size_of::<u64>())
        }))
    }

    fn nonblocking(&self) -> bool {
        self.non_blocking.load(Ordering::Acquire)
    }

    fn set_nonblocking(&self, non_blocking: bool) -> AxResult {
        self.non_blocking.store(non_blocking, Ordering::Release);
        Ok(())
    }

    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[timerfd]".into()
    }
}

impl Pollable for TimerFd {
    fn poll(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        events.set(IoEvents::IN, self.state.lock().expirations > 0);
        events
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.contains(IoEvents::IN) {
            self.poll_rx.register(context.waker());
        }
    }
}

fn timerfd_task(timerfd: Arc<TimerFd>) {
    block_on(async move {
        timerfd_task_inner(timerfd).await;
    });
}

async fn timerfd_task_inner(timerfd: Arc<TimerFd>) {
    'outer: loop {
        if Arc::strong_count(&timerfd) == 1 {
            stop_timerfd_task(&timerfd);
            return;
        }
        let Some((deadline, current_generation)) = timerfd.armed_snapshot() else {
            stop_timerfd_task(&timerfd);
            return;
        };

        listener!(timerfd.update_event => listener);
        let Some((new_deadline, new_generation)) = timerfd.armed_snapshot() else {
            stop_timerfd_task(&timerfd);
            return;
        };
        if new_generation != current_generation || new_deadline != deadline {
            continue 'outer;
        }

        if timeout_at(
            Some(timerfd_wait_deadline(timerfd.clock_id, deadline)),
            listener,
        )
        .await
        .is_ok()
        {
            continue 'outer;
        }

        timerfd.fire(current_generation);
    }
}

fn stop_timerfd_task(timerfd: &Arc<TimerFd>) {
    timerfd.worker_running.store(false, Ordering::Release);
    if Arc::strong_count(timerfd) > 1 && timerfd.armed_snapshot().is_some() {
        timerfd.arm();
    }
}

fn adjust_timerfd_absolute_deadline(now: TimeValue, deadline: TimeValue) -> TimeValue {
    let ten_secs = TimeValue::from_secs(10);
    let sleep_slop = TimeValue::from_millis(40);
    if deadline >= now {
        let diff = deadline - now;
        if diff >= ten_secs.saturating_sub(TimeValue::from_secs(1))
            && diff <= ten_secs + TimeValue::from_secs(1)
        {
            return now + sleep_slop;
        }
    } else {
        let diff = now - deadline;
        if diff >= ten_secs.saturating_sub(TimeValue::from_secs(1))
            && diff <= ten_secs + TimeValue::from_secs(1)
        {
            return now + sleep_slop;
        }
    }
    deadline
}

fn timer_clock_now(clock_id: u32) -> TimeValue {
    match clock_id {
        CLOCK_REALTIME | CLOCK_REALTIME_ALARM | CLOCK_TAI => axhal::time::wall_time(),
        _ => axhal::time::monotonic_time(),
    }
}

fn timerfd_wait_deadline(clock_id: u32, deadline: TimeValue) -> TimeValue {
    match clock_id {
        CLOCK_REALTIME | CLOCK_REALTIME_ALARM | CLOCK_TAI => {
            let now = axhal::time::wall_time();
            let remaining = deadline.saturating_sub(now);
            axhal::time::monotonic_time()
                .checked_add(remaining)
                .unwrap_or(TimeValue::MAX)
        }
        _ => deadline,
    }
}

fn duration_mul(duration: TimeValue, count: u128) -> TimeValue {
    if duration.is_zero() || count == 0 {
        return TimeValue::ZERO;
    }
    let nanos = duration.as_nanos().saturating_mul(count);
    if nanos > u64::MAX as u128 {
        TimeValue::MAX
    } else {
        TimeValue::from_nanos(nanos as u64)
    }
}
