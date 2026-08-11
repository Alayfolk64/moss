//! Sliding forward-progress lease for the LS2K1000 hardware watchdog.

use alloc::string::String;
use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use crate::task::AsThread;

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const RUNTIME_TIMEOUT_SECS: u32 = 30;
const PROGRESS_LEASE_SECS: u64 = 20 * 60;
const CPU_PROGRESS_PERMILLE_ONE_CORE: u64 = 100;

static LAST_EXPLICIT_PROGRESS_NS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
struct UserTaskSet {
    count: usize,
    ids: u64,
}

fn user_task_set() -> UserTaskSet {
    let mut set = UserTaskSet { count: 0, ids: 0 };
    for task in crate::task::tasks() {
        if task.try_as_thread().is_some() {
            set.count += 1;
            set.ids ^= task.id().as_u64().rotate_left(17);
        }
    }
    set
}

fn sample_user_cpu_work(
    now_ns: u64,
    last_sample_ns: &mut u64,
    last_user_ns: &mut [u64; axconfig::plat::CPU_NUM],
) -> u64 {
    let interval_ns = now_ns.saturating_sub(*last_sample_ns);
    if interval_ns == 0 {
        return 0;
    }

    let mut user_delta_ns = 0u64;
    for cpu_id in 0..axtask::active_cpu_count() {
        let Some(user_ns) = axtask::cpu_user_task_time_nanos(cpu_id) else {
            continue;
        };
        user_delta_ns = user_delta_ns.saturating_add(
            user_ns
                .saturating_sub(last_user_ns[cpu_id])
                .min(interval_ns),
        );
        last_user_ns[cpu_id] = user_ns;
    }
    *last_sample_ns = now_ns;
    ((user_delta_ns as u128) * 1_000 / interval_ns as u128) as u64
}

#[inline]
pub(crate) fn note_progress() {
    LAST_EXPLICIT_PROGRESS_NS.store(axhal::time::monotonic_time_nanos(), Ordering::Release);
    axhal::watchdog::ping();
}

pub(crate) fn spawn_monitor() {
    if !axhal::watchdog::start(RUNTIME_TIMEOUT_SECS) {
        return;
    }
    note_progress();
    axtask::spawn_raw(
        monitor,
        String::from("watchdog-monitor"),
        axconfig::TASK_STACK_SIZE,
    );
}

fn monitor() {
    let started_ns = axhal::time::monotonic_time_nanos();
    let mut last_progress_ns = LAST_EXPLICIT_PROGRESS_NS
        .load(Ordering::Acquire)
        .max(started_ns);
    let mut tasks = user_task_set();
    let mut sample_ns = started_ns;
    let mut user_ns =
        core::array::from_fn(|cpu_id| axtask::cpu_user_task_time_nanos(cpu_id).unwrap_or(0));
    let mut expired = false;

    info!(
        "LS2K1000 watchdog active: hardware={} s, no-progress lease={} s",
        RUNTIME_TIMEOUT_SECS, PROGRESS_LEASE_SECS
    );

    loop {
        axtask::sleep(CHECK_INTERVAL);
        let now_ns = axhal::time::monotonic_time_nanos();

        let explicit_ns = LAST_EXPLICIT_PROGRESS_NS.load(Ordering::Acquire);
        if explicit_ns > last_progress_ns {
            last_progress_ns = explicit_ns;
        }

        let current_tasks = user_task_set();
        if current_tasks != tasks {
            tasks = current_tasks;
            last_progress_ns = now_ns;
        }

        if sample_user_cpu_work(now_ns, &mut sample_ns, &mut user_ns)
            >= CPU_PROGRESS_PERMILLE_ONE_CORE
        {
            last_progress_ns = now_ns;
        }

        let idle_ns = now_ns.saturating_sub(last_progress_ns);
        if expired || idle_ns >= PROGRESS_LEASE_SECS * 1_000_000_000 {
            if !expired {
                expired = true;
                warn!(
                    "LS2K1000 watchdog lease expired after {} s without user progress; reset \
                     follows within {} s",
                    idle_ns / 1_000_000_000,
                    RUNTIME_TIMEOUT_SECS
                );
            }
            continue;
        }

        axhal::watchdog::ping();
    }
}

pub(crate) fn stop() {
    axhal::watchdog::stop();
}
