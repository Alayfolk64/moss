//! Forward-progress policy for the LS2K1000 hardware watchdog.

use alloc::string::String;
use core::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use axtask::TaskState;

use crate::task::{AsThread, TaskStat};

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const ORPHAN_MUTEX_GRACE_NS: u64 = 10_000_000_000;
// The hardware counter tops out at about 34 seconds. Use 30 seconds so brief
// scheduler latency during cagent inference or page-cache I/O cannot look like
// a hard lock, while a genuinely wedged scheduler still resets autonomously.
const RUNTIME_TIMEOUT_SECS: u32 = 30;
const PROGRESS_LEASE_SECS: u64 = 20 * 60;
const BUILD_TASK_SNAPSHOT_INTERVAL_SECS: u64 = 5 * 60;
// A compiler using at least 10% of one core during the one-second sampling
// interval is making CPU progress. Periodic futex polling stays below this
// floor and therefore cannot keep a genuinely stalled build alive forever.
const CPU_WORK_PROGRESS_PERMILLE_ONE_CORE: u64 = 100;

static STALL_REPORTED: AtomicBool = AtomicBool::new(false);
static LAST_EXPLICIT_PROGRESS_NS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
struct UserProgressSignature {
    task_count: usize,
    task_ids: u64,
    schedule_count: u64,
}

/// Summarizes actual user-task execution without treating raw syscall volume
/// as progress. A task creation/exit changes the identity set, while CPU work,
/// wakeups and completed I/O change the accumulated resume count.
fn user_progress_signature() -> UserProgressSignature {
    let mut signature = UserProgressSignature {
        task_count: 0,
        task_ids: 0,
        schedule_count: 0,
    };
    for task in crate::task::tasks() {
        if task.try_as_thread().is_none() {
            continue;
        }
        signature.task_count += 1;
        signature.task_ids ^= task.id().as_u64().rotate_left(17);
        signature.schedule_count = signature.schedule_count.wrapping_add(task.schedule_count());
    }
    signature
}

fn user_identity_changed(
    previous: UserProgressSignature,
    current: UserProgressSignature,
) -> bool {
    previous.task_count != current.task_count || previous.task_ids != current.task_ids
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
    let active_cpus = axtask::active_cpu_count();
    let mut user_delta_ns = 0u64;
    for cpu_id in 0..active_cpus {
        let Some(user_ns) = axtask::cpu_user_task_time_nanos(cpu_id) else {
            continue;
        };
        user_delta_ns = user_delta_ns
            .saturating_add(user_ns.saturating_sub(last_user_ns[cpu_id]).min(interval_ns));
        last_user_ns[cpu_id] = user_ns;
    }
    *last_sample_ns = now_ns;
    ((user_delta_ns as u128) * 1_000 / interval_ns as u128) as u64
}

fn report_build_tasks(elapsed_s: u64) {
    let mut matched = 0usize;
    for task in crate::task::tasks() {
        let name = task.name();
        if name != "cargo"
            && name != "rustc"
            && name != "build-script-build"
            && name != "llvm-config"
            && name != "clang"
            && name != "rustfmt"
        {
            continue;
        }

        let Ok(stat) = TaskStat::from_thread(&task) else {
            continue;
        };
        let (syscall, syscall_args) = task
            .try_as_thread()
            .and_then(|thread| thread.watchdog_syscall_snapshot())
            .map_or((None, [0; 3]), |(_, sysno, args)| (Some(sysno), args));
        matched += 1;
        warn!(
            "LS2K1000 build task: elapsed_s={} pid={} tid={} ppid={} pgrp={} name={:?} state={:?} \
             cpu={} schedule_count={} utime_ticks={} stime_ticks={} syscall={:?} \
             args=[{:#x}, {:#x}, {:#x}]",
            elapsed_s,
            stat.pid,
            task.id().as_u64(),
            stat.ppid,
            stat.pgrp,
            name,
            task.state(),
            task.cpu_id(),
            task.schedule_count(),
            stat.utime,
            stat.stime,
            syscall,
            syscall_args[0],
            syscall_args[1],
            syscall_args[2],
        );
    }
    if matched == 0 {
        warn!("LS2K1000 build task snapshot: elapsed_s={elapsed_s}, no build tasks");
    }
}

fn report_memory(elapsed_s: u64) {
    const MIB: usize = 1024 * 1024;
    let allocator = axalloc::global_allocator();
    let usages = allocator.usages();
    let swap = crate::swap::stats();
    warn!(
        "LS2K1000 memory: elapsed_s={} free_mib={} used_mib={} heap_used_mib={} heap_free_mib={} \
         rust_heap_mib={} virt_mem_mib={} page_cache_mib={} page_table_mib={} dma_mib={} \
         swap_used_mib={} swap_total_mib={} swap_out={} swap_in={} swap_probes={} \
         swap_refaults={} swap_io_errors={} swap_bg_passes={} swap_bg_pages={} swap_sync_pages={} \
         large_folio_alloc_failures={}",
        elapsed_s,
        allocator.available_pages() * memory_addr::PAGE_SIZE_4K / MIB,
        allocator.used_pages() * memory_addr::PAGE_SIZE_4K / MIB,
        allocator.used_bytes() / MIB,
        allocator.available_bytes() / MIB,
        usages.get(axalloc::UsageKind::RustHeap) / MIB,
        usages.get(axalloc::UsageKind::VirtMem) / MIB,
        usages.get(axalloc::UsageKind::PageCache) / MIB,
        usages.get(axalloc::UsageKind::PageTable) / MIB,
        usages.get(axalloc::UsageKind::Dma) / MIB,
        swap.used_pages * memory_addr::PAGE_SIZE_4K / MIB,
        swap.total_pages * memory_addr::PAGE_SIZE_4K / MIB,
        swap.swap_out,
        swap.swap_in,
        swap.probes,
        swap.refaults,
        swap.io_errors,
        swap.background_passes,
        swap.background_reclaimed,
        swap.synchronous_reclaimed,
        axfs::large_folio_allocation_failures(),
    );
}

fn report_cpu_utilization(
    elapsed_s: u64,
    now_ns: u64,
    last_sample_ns: &mut u64,
    last_idle_ns: &mut [u64; axconfig::plat::CPU_NUM],
) {
    let interval_ns = now_ns.saturating_sub(*last_sample_ns);
    if interval_ns == 0 {
        return;
    }
    for cpu_id in 0..axtask::active_cpu_count() {
        let Some(idle_ns) = axtask::cpu_idle_time_nanos(cpu_id) else {
            continue;
        };
        let idle_delta_ns = idle_ns.saturating_sub(last_idle_ns[cpu_id]);
        // WFI accounting can straddle a sample boundary by a few timer ticks.
        // Clamp it so the displayed percentage always remains meaningful.
        let busy_ns = interval_ns.saturating_sub(idle_delta_ns.min(interval_ns));
        let busy_permille =
            ((busy_ns as u128) * 1_000 / interval_ns as u128).min(1_000) as u64;
        warn!(
            "LS2K1000 cpu utilization: elapsed_s={} interval_s={} cpu={} \
             busy_percent={}.{} idle_ms={}",
            elapsed_s,
            interval_ns / 1_000_000_000,
            cpu_id,
            busy_permille / 10,
            busy_permille % 10,
            idle_delta_ns / 1_000_000,
        );
        last_idle_ns[cpu_id] = idle_ns;
    }
    *last_sample_ns = now_ns;
}

/// Clears the per-thread syscall marker even when the dispatch function adds
/// a new early return later.  A panic does not need cleanup because the panic
/// path resets the board immediately.
pub(crate) struct SyscallGuard;

impl Drop for SyscallGuard {
    fn drop(&mut self) {
        if let Some(thread) = axtask::current().try_as_thread() {
            thread.watchdog_exit_syscall();
        }
    }
}

#[inline]
pub(crate) fn enter_syscall(sysno: usize, args: [usize; 3]) -> SyscallGuard {
    axtask::current()
        .as_thread()
        .watchdog_enter_syscall(sysno, args, axhal::time::monotonic_time_nanos());
    SyscallGuard
}

#[inline]
pub(crate) fn note_progress() {
    // Boot and controlled-shutdown milestones renew the same sliding lease as
    // user-task execution. Runtime health is not inferred from syscall volume.
    LAST_EXPLICIT_PROGRESS_NS.store(axhal::time::monotonic_time_nanos(), Ordering::Release);
    axhal::watchdog::ping();
}

pub(crate) fn spawn_monitor() {
    // Leave the platform's 30-second boot timeout in force until the monitor
    // task actually gets CPU time.  The monitor shortens it only after the
    // scheduler has proved that this task can run.
    note_progress();
    axtask::spawn_raw(
        monitor,
        String::from("watchdog-monitor"),
        axconfig::TASK_STACK_SIZE,
    );
}

fn monitor() {
    let runtime_watchdog = axhal::watchdog::start(RUNTIME_TIMEOUT_SECS);
    axhal::watchdog::ping();
    info!(
        "LS2K1000 watchdog monitor started: check=1 s, orphan-mutex grace=10 s, runtime reset={} \
         s, no-progress lease={} s, armed={}",
        RUNTIME_TIMEOUT_SECS, PROGRESS_LEASE_SECS, runtime_watchdog
    );

    let started_ns = axhal::time::monotonic_time_nanos();
    let mut last_progress_ns = LAST_EXPLICIT_PROGRESS_NS
        .load(Ordering::Acquire)
        .max(started_ns);
    let mut user_progress = user_progress_signature();
    let mut lease_expired = false;
    let mut clock_reported = false;
    let mut last_cpu_sample_ns = started_ns;
    let mut last_cpu_idle_ns = core::array::from_fn(|cpu_id| {
        axtask::cpu_idle_time_nanos(cpu_id).unwrap_or(0)
    });
    let mut lease_cpu_sample_ns = started_ns;
    let mut lease_cpu_user_ns = core::array::from_fn(|cpu_id| {
        axtask::cpu_user_task_time_nanos(cpu_id).unwrap_or(0)
    });
    let mut next_build_snapshot_ns = started_ns + BUILD_TASK_SNAPSHOT_INTERVAL_SECS * 1_000_000_000;

    loop {
        axtask::sleep(CHECK_INTERVAL);

        let now_ns = axhal::time::monotonic_time_nanos();
        let elapsed_ns = now_ns.saturating_sub(started_ns);
        let explicit_progress_ns = LAST_EXPLICIT_PROGRESS_NS.load(Ordering::Acquire);
        if explicit_progress_ns > last_progress_ns {
            last_progress_ns = explicit_progress_ns;
        }
        let current_user_progress = user_progress_signature();
        if user_identity_changed(user_progress, current_user_progress) {
            last_progress_ns = now_ns;
        }
        user_progress = current_user_progress;
        let user_cpu_work_permille_one_core = sample_user_cpu_work(
            now_ns,
            &mut lease_cpu_sample_ns,
            &mut lease_cpu_user_ns,
        );
        if user_cpu_work_permille_one_core >= CPU_WORK_PROGRESS_PERMILLE_ONE_CORE {
            last_progress_ns = now_ns;
        }
        if now_ns >= next_build_snapshot_ns {
            report_memory(elapsed_ns / 1_000_000_000);
            report_cpu_utilization(
                elapsed_ns / 1_000_000_000,
                now_ns,
                &mut last_cpu_sample_ns,
                &mut last_cpu_idle_ns,
            );
            report_build_tasks(elapsed_ns / 1_000_000_000);
            warn!(
                "LS2K1000 progress lease: elapsed_s={} idle_s={} user_tasks={} schedule_count={} \
                 user_cpu_work_permille_one_core={}",
                elapsed_ns / 1_000_000_000,
                now_ns.saturating_sub(last_progress_ns) / 1_000_000_000,
                user_progress.task_count,
                user_progress.schedule_count,
                user_cpu_work_permille_one_core,
            );
            while next_build_snapshot_ns <= now_ns {
                next_build_snapshot_ns = next_build_snapshot_ns
                    .saturating_add(BUILD_TASK_SNAPSHOT_INTERVAL_SECS * 1_000_000_000);
            }
        }
        if !clock_reported {
            clock_reported = true;
            info!(
                "LS2K1000 watchdog clock baseline: started_ns={}, now_ns={}, elapsed_ms={}",
                started_ns,
                now_ns,
                elapsed_ns / 1_000_000
            );
        }
        if lease_expired
            || now_ns.saturating_sub(last_progress_ns) >= PROGRESS_LEASE_SECS * 1_000_000_000
        {
            if !lease_expired {
                lease_expired = true;
                report_memory(elapsed_ns / 1_000_000_000);
                report_build_tasks(elapsed_ns / 1_000_000_000);
                warn!(
                    "LS2K1000 watchdog progress lease expired: elapsed_s={} idle_s={}, \
                     user_tasks={} schedule_count={}; withholding ping, hardware reset follows \
                     within {} s",
                    elapsed_ns / 1_000_000_000,
                    now_ns.saturating_sub(last_progress_ns) / 1_000_000_000,
                    user_progress.task_count,
                    user_progress.schedule_count,
                    RUNTIME_TIMEOUT_SECS
                );
            }
            // Expiration is deliberately irreversible for this boot. If the
            // workload wakes during the hardware timeout, it must not turn a
            // timed-out experiment back into an unbounded one.
            continue;
        }
        let mut suspect: Option<(
            u64,
            u32,
            String,
            usize,
            u64,
            usize,
            u64,
            TaskState,
            u64,
            Option<usize>,
        )> = None;
        for task in crate::task::tasks() {
            let state = task.state();
            if !matches!(state, TaskState::Running | TaskState::Ready) {
                continue;
            }
            let Some((started_ns, mutex_address, owner_id, caller)) = task.mutex_wait_snapshot()
            else {
                continue;
            };
            let age_ns = now_ns.saturating_sub(started_ns);
            if age_ns <= ORPHAN_MUTEX_GRACE_NS {
                continue;
            }

            let owner_is_gone =
                axtask::task_by_id(owner_id).is_none_or(|owner| owner.state() == TaskState::Exited);
            if !owner_is_gone {
                continue;
            }

            let replace = suspect
                .as_ref()
                .map_or(true, |(_, _, _, _, _, _, oldest_age_ns, ..)| {
                    age_ns > *oldest_age_ns
                });
            if replace {
                let (pid, sysno) = task.try_as_thread().map_or((0, None), |thread| {
                    (
                        thread.proc_data.proc.pid(),
                        thread
                            .watchdog_syscall_snapshot()
                            .map(|(_, sysno, _)| sysno),
                    )
                });
                suspect = Some((
                    task.id().as_u64(),
                    pid,
                    task.name(),
                    mutex_address,
                    owner_id,
                    caller,
                    age_ns,
                    state,
                    task.schedule_count(),
                    sysno,
                ));
            }
        }

        if let Some((
            tid,
            pid,
            name,
            mutex_address,
            owner_id,
            caller,
            age_ns,
            state,
            schedule_count,
            sysno,
        )) = suspect
        {
            if !STALL_REPORTED.swap(true, Ordering::AcqRel) {
                let caller = (caller != 0)
                    .then(|| unsafe { &*(caller as *const core::panic::Location<'static>) });
                warn!(
                    "LS2K1000 watchdog detected orphaned cooperative mutex: pid={} tid={} \
                     name={:?} mutex={:#x} caller={:?} dead_owner_tid={} wait_ms={} state={:?} \
                     schedule_count={} syscall={:?}; withholding ping, hardware reset follows \
                     within {} s if the wait persists",
                    pid,
                    tid,
                    name,
                    mutex_address,
                    caller,
                    owner_id,
                    age_ns / 1_000_000,
                    state,
                    schedule_count,
                    sysno,
                    RUNTIME_TIMEOUT_SECS
                );
            }
            // Only an orphaned mutex is actionable here. A long wait behind a
            // live owner is ordinary contention and must keep feeding the
            // watchdog, even when the owner is currently blocked.
            continue;
        }

        if STALL_REPORTED.swap(false, Ordering::AcqRel) {
            warn!("LS2K1000 orphaned mutex cleared; watchdog feeding restored");
        }

        // Long userspace computation and blocking syscalls are healthy. A
        // scheduler/IRQ hard stall prevents this monitor from running at all,
        // so the hardware watchdog still resets the board without heuristics.
        axhal::watchdog::ping();
    }
}

pub(crate) fn stop() {
    axhal::watchdog::stop();
    info!("LS2K1000 watchdog stopped for controlled shutdown");
}
