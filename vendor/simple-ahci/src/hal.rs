pub trait Hal {
    /// Convert a virtual address to a physical address.
    fn virt_to_phys(va: usize) -> usize;

    /// Current time in milliseconds
    fn current_ms() -> u64;

    /// Flush the Dcache.
    fn flush_dcache();

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

pub(crate) fn wait_until_timeout<H: Hal>(cond: impl Fn() -> bool, timeout: u64) -> bool {
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
