use alloc::{string::ToString, vec::Vec};
use core::time::Duration;

use axtask::SchedulerStats;

const REPORT_INTERVAL: Duration = Duration::from_secs(30);

fn delta(current: u64, previous: u64) -> u64 {
    current.wrapping_sub(previous)
}

fn report(current: &[SchedulerStats], previous: &[SchedulerStats]) {
    for stat in current {
        let old = previous.get(stat.cpu_id).copied().unwrap_or_default();
        let ticks = delta(stat.ticks, old.ticks);
        let busy = delta(stat.busy_ticks, old.busy_ticks);
        let utilization = if ticks == 0 { 0 } else { busy * 100 / ticks };

        warn!(
            "[sched] cpu={} busy={:3}% ready={} switch={} wake={} migrate={} ipi={} idle={}",
            stat.cpu_id,
            utilization,
            stat.ready_tasks,
            delta(stat.context_switches, old.context_switches),
            delta(stat.wakeups, old.wakeups),
            delta(stat.migrations, old.migrations),
            delta(stat.ipi_kicks, old.ipi_kicks),
            delta(stat.idle_selections, old.idle_selections),
        );
    }
}

fn scheduler_stats_task() {
    let mut previous: Vec<SchedulerStats> = axtask::scheduler_stats();
    loop {
        axtask::sleep(REPORT_INTERVAL);
        let current = axtask::scheduler_stats();
        report(&current, &previous);
        previous = current;
    }
}

pub fn spawn_scheduler_stats_task() {
    axtask::spawn_raw(
        scheduler_stats_task,
        "scheduler_stats".to_string(),
        axconfig::TASK_STACK_SIZE,
    );
}
