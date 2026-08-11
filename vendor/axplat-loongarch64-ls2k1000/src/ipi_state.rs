//! Pure helpers for the LS2K1000 IOCSR IPI action protocol.

/// Linux-compatible action masks. The value passed to `send_ipi_single` is a
/// bit mask; the hardware writes each set bit's index into IPI_SEND[4:0].
pub(crate) const BOOT_ACTION: u32 = 1 << 0;
pub(crate) const RUNTIME_ACTION: u32 = 1 << 1;
pub(crate) const ENABLED_ACTIONS: u32 = BOOT_ACTION | RUNTIME_ACTION;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PendingActions {
    pub(crate) runtime: bool,
    pub(crate) unknown: u32,
}

/// Decodes the status latched by the per-core IOCSR IPI controller.
///
/// The boot action only wakes a secondary CPU. It has no `axipi` queue entry
/// and is therefore acknowledged without dispatching the runtime handler.
pub(crate) const fn decode_pending(status: u32) -> PendingActions {
    PendingActions {
        runtime: status & RUNTIME_ACTION != 0,
        unknown: status & !ENABLED_ACTIONS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_and_runtime_actions_use_distinct_vectors() {
        assert_eq!(BOOT_ACTION, 1 << 0);
        assert_eq!(RUNTIME_ACTION, 1 << 1);
        assert_eq!(BOOT_ACTION & RUNTIME_ACTION, 0);
    }

    #[test]
    fn boot_action_does_not_dispatch_runtime_queue() {
        let pending = decode_pending(BOOT_ACTION);
        assert!(!pending.runtime);
        assert_eq!(pending.unknown, 0);
    }

    #[test]
    fn runtime_action_is_dispatched_even_with_boot_action_pending() {
        let pending = decode_pending(BOOT_ACTION | RUNTIME_ACTION);
        assert!(pending.runtime);
        assert_eq!(pending.unknown, 0);
    }

    #[test]
    fn unsupported_actions_are_reported_without_becoming_runtime_work() {
        let unsupported = 1 << 7;
        let pending = decode_pending(BOOT_ACTION | unsupported);
        assert!(!pending.runtime);
        assert_eq!(pending.unknown, unsupported);
    }
}
