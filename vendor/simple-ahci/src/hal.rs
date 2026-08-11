use core::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WaitMode {
    Poll,
    Event,
}

pub(crate) struct WaitModeState {
    event_waits_enabled: AtomicBool,
}

impl WaitModeState {
    pub(crate) const fn new_polling() -> Self {
        Self {
            event_waits_enabled: AtomicBool::new(false),
        }
    }

    pub(crate) fn current(&self) -> WaitMode {
        if self.event_waits_enabled.load(Ordering::Acquire) {
            WaitMode::Event
        } else {
            WaitMode::Poll
        }
    }

    pub(crate) fn enable_event_waits(&self) {
        self.event_waits_enabled.store(true, Ordering::Release);
    }

    /// Restore boot-time polling after the containing port reaches its final
    /// unpublished allocation.
    pub(crate) fn reinitialize_unpublished(&mut self) {
        self.event_waits_enabled.store(false, Ordering::Relaxed);
    }
}

pub trait Hal {
    /// Convert a virtual address to a physical address.
    fn virt_to_phys(va: usize) -> usize;

    /// Current time in milliseconds
    fn current_ms() -> u64;

    /// Flush the Dcache.
    fn flush_dcache();

    /// Save the local interrupt state and mask local interrupts for the short
    /// COMRESET assertion delay.
    ///
    /// Platforms that do not provide local interrupt control may keep the
    /// default no-op implementation. The returned value is passed back to
    /// [`Hal::restore_local_irqs`] unchanged.
    fn save_and_mask_local_irqs() -> bool {
        false
    }

    /// Restore the local interrupt state saved by
    /// [`Hal::save_and_mask_local_irqs`].
    fn restore_local_irqs(_was_enabled: bool) {}

    /// Whether the current execution context may block on a wait queue.
    fn can_sleep() -> bool {
        false
    }

    /// Wait until `condition` becomes true or `timeout_ms` elapses.
    ///
    /// A kernel integration should block the current task and arrange for
    /// [`Hal::notify_event`] to wake it. The default keeps compatibility with
    /// simple bare-metal users that do not have a scheduler.
    fn wait_for_event<F>(timeout_ms: u64, mut condition: F) -> bool
    where
        F: FnMut() -> bool,
    {
        let start = Self::current_ms();
        loop {
            if condition() {
                return true;
            }
            if Self::current_ms().saturating_sub(start) >= timeout_ms {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    /// Wake tasks sleeping for command-slot or completion events.
    fn notify_event() {}
}

pub(crate) fn wait_until_timeout<H: Hal>(mut cond: impl FnMut() -> bool, timeout: u64) -> bool {
    let start = H::current_ms();
    loop {
        if cond() {
            return true;
        }
        if H::current_ms().saturating_sub(start) >= timeout {
            return false;
        }
        core::hint::spin_loop();
    }
}

pub(crate) fn wait_for_event_or_spin<H: Hal, F>(mode: WaitMode, timeout: u64, condition: F) -> bool
where
    F: FnMut() -> bool,
{
    if mode == WaitMode::Event && H::can_sleep() {
        H::wait_for_event(timeout, condition)
    } else {
        wait_until_timeout::<H>(condition, timeout)
    }
}

/// Delay without busy-spinning once the scheduler can safely block the current
/// task. The false condition intentionally makes the selected wait run to its
/// timeout.
pub(crate) fn delay_ms<H: Hal>(mode: WaitMode, milliseconds: u64) {
    let _ = wait_for_event_or_spin::<H, _>(mode, milliseconds, || false);
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use super::{Hal, WaitMode, WaitModeState, wait_for_event_or_spin};

    static NOW: AtomicU64 = AtomicU64::new(0);
    static SLEPT: AtomicBool = AtomicBool::new(false);

    struct BootHal;

    impl Hal for BootHal {
        fn virt_to_phys(va: usize) -> usize {
            va
        }

        fn current_ms() -> u64 {
            NOW.fetch_add(1, Ordering::Relaxed)
        }

        fn flush_dcache() {}

        fn wait_for_event<F>(_timeout_ms: u64, _condition: F) -> bool
        where
            F: FnMut() -> bool,
        {
            panic!("boot context must not sleep")
        }
    }

    struct TaskHal;

    impl Hal for TaskHal {
        fn virt_to_phys(va: usize) -> usize {
            va
        }

        fn current_ms() -> u64 {
            NOW.fetch_add(1, Ordering::Relaxed)
        }

        fn flush_dcache() {}

        fn can_sleep() -> bool {
            true
        }

        fn wait_for_event<F>(_timeout_ms: u64, mut condition: F) -> bool
        where
            F: FnMut() -> bool,
        {
            SLEPT.store(true, Ordering::Relaxed);
            condition()
        }
    }

    struct SleepCapablePollHal;

    impl Hal for SleepCapablePollHal {
        fn virt_to_phys(va: usize) -> usize {
            va
        }

        fn current_ms() -> u64 {
            NOW.fetch_add(1, Ordering::Relaxed)
        }

        fn flush_dcache() {}

        fn can_sleep() -> bool {
            true
        }

        fn wait_for_event<F>(_timeout_ms: u64, _condition: F) -> bool
        where
            F: FnMut() -> bool,
        {
            panic!("explicit polling mode must not enter the sleeping wait hook")
        }
    }

    #[test]
    fn boot_context_polls_without_entering_the_wait_queue() {
        NOW.store(0, Ordering::Relaxed);
        let mut attempts = 0;
        assert!(wait_for_event_or_spin::<BootHal, _>(
            WaitMode::Poll,
            10,
            || {
                attempts += 1;
                attempts == 3
            }
        ));
        assert_eq!(attempts, 3);
    }

    #[test]
    fn explicit_poll_mode_overrides_a_sleep_capable_hal() {
        NOW.store(0, Ordering::Relaxed);
        let mut attempts = 0;
        assert!(wait_for_event_or_spin::<SleepCapablePollHal, _>(
            WaitMode::Poll,
            10,
            || {
                attempts += 1;
                attempts == 3
            }
        ));
        assert_eq!(attempts, 3);
    }

    #[test]
    fn controller_wait_mode_changes_only_after_probe_finishes() {
        let mut state = WaitModeState::new_polling();
        assert_eq!(state.current(), WaitMode::Poll);

        state.enable_event_waits();

        assert_eq!(state.current(), WaitMode::Event);
        state.reinitialize_unpublished();
        assert_eq!(state.current(), WaitMode::Poll);
    }

    #[test]
    fn runtime_task_context_uses_the_sleeping_wait_hook() {
        NOW.store(0, Ordering::Relaxed);
        SLEPT.store(false, Ordering::Relaxed);
        assert!(wait_for_event_or_spin::<TaskHal, _>(
            WaitMode::Event,
            10,
            || true
        ));
        assert!(SLEPT.load(Ordering::Relaxed));
    }
}
