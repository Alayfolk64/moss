/// Returns whether a task context may enter a blocking wait.
///
/// Bootstrap and per-CPU idle contexts use scheduler-owned stacks and must
/// never be put on a wait queue. Ordinary spawned tasks can be blocked and
/// resumed by the scheduler.
pub(crate) const fn task_context_may_block(is_init: bool, is_idle: bool) -> bool {
    !is_init && !is_idle
}

/// Returns whether `block_on` may park the current task in the scheduler.
///
/// The primary bootstrap task is deliberately allowed to park after boot
/// initialization (for example while joining init). The scheduler-owned idle
/// task never is: it must keep polling until the future becomes ready.
pub(crate) const fn block_on_may_park(is_idle: bool) -> bool {
    !is_idle
}

/// Returns whether a periodic timer tick may preempt this task context.
///
/// The bootstrap task runs on the firmware-provided boot stack. It may switch
/// voluntarily once initialization reaches an explicit block/yield point, but
/// it must not be suspended from an IRQ return path. Spawned tasks own normal
/// scheduler stacks and remain fully preemptible.
pub(crate) const fn timer_tick_may_preempt(is_init: bool, is_idle: bool) -> bool {
    !is_init && !is_idle
}

#[cfg(test)]
mod tests {
    use super::{block_on_may_park, task_context_may_block, timer_tick_may_preempt};

    #[test]
    fn only_spawned_non_idle_tasks_may_block() {
        assert!(!task_context_may_block(true, false));
        assert!(!task_context_may_block(true, true));
        assert!(!task_context_may_block(false, true));
        assert!(task_context_may_block(false, false));
    }

    #[test]
    fn block_on_never_parks_the_idle_task() {
        assert!(!block_on_may_park(true));
        assert!(block_on_may_park(false));
    }

    #[test]
    fn timer_tick_does_not_preempt_bootstrap_or_idle_contexts() {
        assert!(!timer_tick_may_preempt(true, false));
        assert!(!timer_tick_may_preempt(true, true));
        assert!(!timer_tick_may_preempt(false, true));
        assert!(timer_tick_may_preempt(false, false));
    }
}
