//! Pure forward-progress policy for the LS2K1000 watchdog monitor.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopReason {
    HardDeadline,
    IdleLease,
    CpuOnlyCap,
}

/// Tracks progress that changes externally visible task state.
///
/// CPU consumption is intentionally not recorded here: a spinning task or a
/// syscall livelock also consumes CPU. The caller may grant CPU-bound work a
/// longer, but still fixed, grace period through `cpu_only_cap_ns`.
pub(crate) struct ForwardProgressLease {
    started_ns: u64,
    last_credible_ns: u64,
}

impl ForwardProgressLease {
    pub(crate) const fn new(started_ns: u64) -> Self {
        Self {
            started_ns,
            last_credible_ns: started_ns,
        }
    }

    pub(crate) fn note_credible_progress(&mut self, now_ns: u64) {
        self.last_credible_ns = self.last_credible_ns.max(now_ns);
    }

    pub(crate) const fn last_credible_ns(&self) -> u64 {
        self.last_credible_ns
    }

    pub(crate) fn stop_reason(
        &self,
        now_ns: u64,
        cpu_active: bool,
        idle_lease_ns: u64,
        cpu_only_cap_ns: u64,
        hard_deadline_ns: Option<u64>,
    ) -> Option<StopReason> {
        let runtime_ns = now_ns.saturating_sub(self.started_ns);
        if hard_deadline_ns.is_some_and(|deadline_ns| runtime_ns >= deadline_ns) {
            return Some(StopReason::HardDeadline);
        }

        let idle_ns = now_ns.saturating_sub(self.last_credible_ns);
        if cpu_active {
            (idle_ns >= cpu_only_cap_ns).then_some(StopReason::CpuOnlyCap)
        } else {
            (idle_ns >= idle_lease_ns).then_some(StopReason::IdleLease)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ForwardProgressLease, StopReason};

    const SECOND: u64 = 1_000_000_000;
    const IDLE_LEASE: u64 = 20 * 60 * SECOND;
    const CPU_ONLY_CAP: u64 = 30 * 60 * SECOND;

    #[test]
    fn idle_workload_expires_at_normal_lease() {
        let lease = ForwardProgressLease::new(10 * SECOND);
        assert_eq!(
            lease.stop_reason(
                10 * SECOND + IDLE_LEASE - 1,
                false,
                IDLE_LEASE,
                CPU_ONLY_CAP,
                None,
            ),
            None
        );
        assert_eq!(
            lease.stop_reason(
                10 * SECOND + IDLE_LEASE,
                false,
                IDLE_LEASE,
                CPU_ONLY_CAP,
                None,
            ),
            Some(StopReason::IdleLease)
        );
    }

    #[test]
    fn continuous_cpu_activity_cannot_renew_forever() {
        let lease = ForwardProgressLease::new(0);
        for minute in 20..30 {
            assert_eq!(
                lease.stop_reason(minute * 60 * SECOND, true, IDLE_LEASE, CPU_ONLY_CAP, None,),
                None
            );
        }
        assert_eq!(
            lease.stop_reason(CPU_ONLY_CAP, true, IDLE_LEASE, CPU_ONLY_CAP, None,),
            Some(StopReason::CpuOnlyCap)
        );
        assert_eq!(lease.last_credible_ns(), 0);
    }

    #[test]
    fn credible_progress_starts_fresh_bounded_leases() {
        let mut lease = ForwardProgressLease::new(0);
        lease.note_credible_progress(29 * 60 * SECOND);

        assert_eq!(
            lease.stop_reason(49 * 60 * SECOND - 1, false, IDLE_LEASE, CPU_ONLY_CAP, None,),
            None
        );
        assert_eq!(
            lease.stop_reason(49 * 60 * SECOND, false, IDLE_LEASE, CPU_ONLY_CAP, None,),
            Some(StopReason::IdleLease)
        );
        assert_eq!(
            lease.stop_reason(59 * 60 * SECOND, true, IDLE_LEASE, CPU_ONLY_CAP, None,),
            Some(StopReason::CpuOnlyCap)
        );
    }

    #[test]
    fn hard_deadline_wins_even_after_credible_progress() {
        let mut lease = ForwardProgressLease::new(5 * SECOND);
        lease.note_credible_progress(9 * 60 * SECOND);
        assert_eq!(
            lease.stop_reason(
                5 * SECOND + 10 * 60 * SECOND,
                true,
                IDLE_LEASE,
                CPU_ONLY_CAP,
                Some(10 * 60 * SECOND),
            ),
            Some(StopReason::HardDeadline)
        );
    }
}
