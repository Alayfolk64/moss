//! Bounded forward-progress lease for unattended hardware-board runs.

mod policy;

use alloc::string::String;
use core::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use policy::{ForwardProgressLease, StopReason};

use crate::task::AsThread;

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const RUNTIME_TIMEOUT_SECS: u32 = 30;
const PROGRESS_LEASE_SECS: u64 = 20 * 60;
const CPU_ONLY_CAP_SECS: u64 = 30 * 60;
const CPU_PROGRESS_PERMILLE_ONE_CORE: u64 = 100;
const TEST_DEADLINE_PROPERTY: &str = "moss,watchdog-hard-deadline-secs";
const NANOS_PER_SEC: u64 = 1_000_000_000;

static LAST_EXPLICIT_PROGRESS_NS: AtomicU64 = AtomicU64::new(0);
static MONITOR_ENABLED: AtomicBool = AtomicBool::new(false);

fn hard_deadline_secs() -> Option<u64> {
    let raw = axhal::dtb::get_chosen_property(TEST_DEADLINE_PROPERTY)?;
    let bytes: [u8; 4] = raw.try_into().ok()?;
    let seconds = u32::from_be_bytes(bytes) as u64;
    (60..=3600).contains(&seconds).then_some(seconds)
}

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
}

pub(crate) fn spawn_monitor() {
    MONITOR_ENABLED.store(true, Ordering::Release);
    if !axhal::watchdog::start(RUNTIME_TIMEOUT_SECS) {
        MONITOR_ENABLED.store(false, Ordering::Release);
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
    let mut lease = ForwardProgressLease::new(started_ns);
    lease.note_credible_progress(LAST_EXPLICIT_PROGRESS_NS.load(Ordering::Acquire));
    let mut tasks = user_task_set();
    let mut sample_ns = started_ns;
    let mut user_ns =
        core::array::from_fn(|cpu_id| axtask::cpu_user_task_time_nanos(cpu_id).unwrap_or(0));
    let mut expired = false;
    let hard_deadline_secs = hard_deadline_secs();
    let hard_deadline_ns = hard_deadline_secs.map(|seconds| seconds * NANOS_PER_SEC);

    info!(
        "Hardware watchdog active: timeout={} s, no-progress lease={} s, CPU-only cap={} s",
        RUNTIME_TIMEOUT_SECS, PROGRESS_LEASE_SECS, CPU_ONLY_CAP_SECS
    );

    loop {
        axtask::sleep(CHECK_INTERVAL);
        if !MONITOR_ENABLED.load(Ordering::Acquire) {
            return;
        }
        let now_ns = axhal::time::monotonic_time_nanos();

        let explicit_ns = LAST_EXPLICIT_PROGRESS_NS.load(Ordering::Acquire);
        if explicit_ns > lease.last_credible_ns() {
            lease.note_credible_progress(explicit_ns);
        }

        let current_tasks = user_task_set();
        if current_tasks != tasks {
            tasks = current_tasks;
            lease.note_credible_progress(now_ns);
        }

        // CPU time by itself is not credible forward progress: a spinning
        // userspace task or syscall livelock also raises this counter. It may
        // select the longer CPU-bound grace period, but can never renew it.
        let cpu_active = sample_user_cpu_work(now_ns, &mut sample_ns, &mut user_ns)
            >= CPU_PROGRESS_PERMILLE_ONE_CORE;

        let idle_ns = now_ns.saturating_sub(lease.last_credible_ns());
        let runtime_ns = now_ns.saturating_sub(started_ns);
        let reason = lease.stop_reason(
            now_ns,
            cpu_active,
            PROGRESS_LEASE_SECS * NANOS_PER_SEC,
            CPU_ONLY_CAP_SECS * NANOS_PER_SEC,
            hard_deadline_ns,
        );
        if expired {
            continue;
        }
        if let Some(reason) = reason {
            expired = true;
            match reason {
                StopReason::HardDeadline => {
                    warn!(
                        "Watchdog hard test deadline expired after {} s; reset follows within {} s",
                        runtime_ns / NANOS_PER_SEC,
                        RUNTIME_TIMEOUT_SECS
                    );
                }
                StopReason::IdleLease => {
                    warn!(
                        "Watchdog lease expired after {} s without semantic progress; reset \
                         follows within {} s",
                        idle_ns / NANOS_PER_SEC,
                        RUNTIME_TIMEOUT_SECS
                    );
                }
                StopReason::CpuOnlyCap => {
                    warn!(
                        "Watchdog CPU-only cap expired after {} s without semantic progress \
                         (possible livelock); reset follows within {} s",
                        idle_ns / NANOS_PER_SEC,
                        RUNTIME_TIMEOUT_SECS
                    );
                }
            }
            continue;
        }

        axhal::watchdog::ping();
    }
}

pub(crate) fn stop() {
    // Publish monitor shutdown before disabling the device.  A reset path may
    // immediately arm a short hardware lease; the old monitor must never wake
    // on another CPU and feed that new lease.
    MONITOR_ENABLED.store(false, Ordering::Release);
    axhal::watchdog::stop();
}
