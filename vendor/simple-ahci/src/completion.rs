//! Lock-free AHCI command completion state shared by submitters and IRQ code.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

const MAX_SLOTS: usize = 32;
const SLOT_FREE: u8 = 0;
const SLOT_ISSUED: u8 = 1;
const SLOT_COMPLETE: u8 = 2;
const SLOT_ERROR: u8 = 3;
const PORT_IRQ_FATAL_MASK: u32 =
    (1 << 30) | (1 << 29) | (1 << 28) | (1 << 27) | (1 << 23) | (1 << 22) | (1 << 6) | (1 << 4);

struct CompletionSlot {
    state: AtomicU8,
    generation: AtomicU32,
}

impl CompletionSlot {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(SLOT_FREE),
            generation: AtomicU32::new(0),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CommandToken {
    pub(crate) slot: usize,
    pub(crate) generation: u32,
}

pub(crate) struct CompletionQueue {
    capacity: usize,
    slots: [CompletionSlot; MAX_SLOTS],
    issued: AtomicU32,
    next_generation: AtomicU32,
    #[cfg(test)]
    publish_pause: AtomicU8,
    #[cfg(test)]
    publish_claim_pause: AtomicU8,
}

/// A FIFO ticket for the single non-NCQ command channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AdmissionTicket(u64);

/// Serializes non-NCQ ATA commands in arrival order. Linux libata enforces the
/// same one-command rule; FIFO ownership additionally prevents a page-out loop
/// from reacquiring the port ahead of filesystem readers on every command.
pub(crate) struct NonNcqAdmission {
    next: AtomicU64,
    serving: AtomicU64,
}

impl NonNcqAdmission {
    pub(crate) const fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
            serving: AtomicU64::new(0),
        }
    }

    pub(crate) fn reserve(&self) -> AdmissionTicket {
        AdmissionTicket(self.next.fetch_add(1, Ordering::AcqRel))
    }

    pub(crate) fn is_turn(&self, ticket: AdmissionTicket) -> bool {
        self.serving.load(Ordering::Acquire) == ticket.0
    }

    pub(crate) fn release(&self, ticket: AdmissionTicket) {
        self.serving
            .compare_exchange(
                ticket.0,
                ticket.0.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .expect("releasing a non-current AHCI admission ticket");
    }

    /// Reset atomics after the containing port reaches its final allocation.
    /// No command may observe the admission object at this point.
    pub(crate) fn reinitialize_unpublished(&mut self) {
        self.next.store(0, Ordering::Relaxed);
        self.serving.store(0, Ordering::Relaxed);
    }
}

impl CompletionQueue {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!((1..=MAX_SLOTS).contains(&capacity));
        Self {
            capacity,
            slots: [const { CompletionSlot::new() }; MAX_SLOTS],
            issued: AtomicU32::new(0),
            next_generation: AtomicU32::new(1),
            #[cfg(test)]
            publish_pause: AtomicU8::new(0),
            #[cfg(test)]
            publish_claim_pause: AtomicU8::new(0),
        }
    }

    /// Reset atomics after the containing port reaches its final allocation.
    /// This is only valid before the IRQ handle or block device is published.
    pub(crate) fn reinitialize_unpublished(&mut self) {
        for slot in &mut self.slots {
            slot.state.store(SLOT_FREE, Ordering::Relaxed);
            slot.generation.store(0, Ordering::Relaxed);
        }
        self.issued.store(0, Ordering::Relaxed);
        self.next_generation.store(1, Ordering::Relaxed);
        #[cfg(test)]
        {
            self.publish_pause.store(0, Ordering::Relaxed);
            self.publish_claim_pause.store(0, Ordering::Relaxed);
        }
    }

    pub(crate) fn try_reserve(&self) -> Option<CommandToken> {
        for slot in 0..self.capacity {
            let entry = &self.slots[slot];
            if entry
                .state
                .compare_exchange(SLOT_FREE, SLOT_ISSUED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
                entry.generation.store(generation, Ordering::Release);
                return Some(CommandToken { slot, generation });
            }
        }
        None
    }

    pub(crate) fn cancel(&self, token: CommandToken) {
        let entry = &self.slots[token.slot];
        if entry.generation.load(Ordering::Acquire) == token.generation {
            self.issued
                .fetch_and(!(1_u32 << token.slot), Ordering::AcqRel);
            entry.state.store(SLOT_FREE, Ordering::Release);
        }
    }

    pub(crate) fn mark_issued(&self, token: CommandToken) {
        debug_assert_eq!(
            self.slots[token.slot].generation.load(Ordering::Acquire),
            token.generation
        );
        self.issued.fetch_or(1_u32 << token.slot, Ordering::Release);
    }

    pub(crate) fn issued(&self) -> u32 {
        self.issued.load(Ordering::Acquire)
    }

    pub(crate) fn publish(&self, mask: u32, failed: bool) -> u32 {
        self.publish_checked(mask, failed, || false)
    }

    /// Claim completion bits exactly once, then let the caller revalidate a
    /// tentative success against device error state observed after the claim.
    /// This closes the interval where a fatal IRQ can no longer claim the bit
    /// but has already made the command's result unreliable.
    pub(crate) fn publish_checked<F>(
        &self,
        mask: u32,
        initially_failed: bool,
        revalidate_failure: F,
    ) -> u32
    where
        F: FnOnce() -> bool,
    {
        // Atomically claim each completion while removing it from the issued
        // bitmap. A separate load followed by fetch_and lets an IRQ publisher
        // and the submitter both claim the same slot, allowing a late success
        // to overwrite ERROR. Clearing after publishing is also unsafe because
        // the waiter may already have reused the slot for a new generation.
        let published = self.issued.fetch_and(!mask, Ordering::AcqRel) & mask;
        #[cfg(test)]
        if published != 0
            && self
                .publish_claim_pause
                .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            while self.publish_claim_pause.load(Ordering::Acquire) != 3 {
                core::hint::spin_loop();
            }
        }
        let failed = initially_failed || (published != 0 && revalidate_failure());
        for slot in 0..self.capacity {
            if published & (1_u32 << slot) != 0 {
                self.slots[slot].state.store(
                    if failed { SLOT_ERROR } else { SLOT_COMPLETE },
                    Ordering::Release,
                );
                #[cfg(test)]
                if self
                    .publish_pause
                    .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    while self.publish_pause.load(Ordering::Acquire) != 3 {
                        core::hint::spin_loop();
                    }
                }
            }
        }
        published
    }

    pub(crate) fn is_done(&self, token: CommandToken) -> bool {
        let entry = &self.slots[token.slot];
        entry.generation.load(Ordering::Acquire) == token.generation
            && matches!(
                entry.state.load(Ordering::Acquire),
                SLOT_COMPLETE | SLOT_ERROR
            )
    }

    pub(crate) fn take(&self, token: CommandToken) -> Option<bool> {
        let entry = &self.slots[token.slot];
        if entry.generation.load(Ordering::Acquire) != token.generation {
            return None;
        }
        let success = match entry.state.load(Ordering::Acquire) {
            SLOT_COMPLETE => true,
            SLOT_ERROR => false,
            _ => return None,
        };
        entry.state.store(SLOT_FREE, Ordering::Release);
        Some(success)
    }
}

pub(crate) fn completed_commands(issued: u32, command_issue: u32, fatal: bool) -> u32 {
    if fatal {
        issued
    } else {
        issued & !command_issue
    }
}

pub(crate) const fn port_interrupt_failed(status_bits: u32) -> bool {
    status_bits & PORT_IRQ_FATAL_MASK != 0
}

/// Classify a command after its issued state becomes visible. A fatal IRQ can
/// run between the PxCI write and that publication, acknowledge PxIS while the
/// issued bitmap is still empty, and leave only `recovery_needed` as evidence.
pub(crate) const fn command_completion_failed(
    recovery_needed: bool,
    status_bits: u32,
    task_file_error: bool,
) -> bool {
    recovery_needed || port_interrupt_failed(status_bits) || task_file_error
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryAction {
    Admit,
    Wait,
    Defer,
    Recover,
    Fail,
}

/// Decide command admission before touching a slot. Fatal/error interrupts may
/// arrive while the port is idle, so pending recovery must be driven by the
/// next submitter rather than treated as an ordinary unavailable slot.
pub(crate) const fn recovery_action(
    controller_failed: bool,
    recovering: bool,
    recovery_needed: bool,
    now_ms: u64,
    retry_after_ms: u64,
) -> RecoveryAction {
    if controller_failed {
        RecoveryAction::Fail
    } else if recovering {
        RecoveryAction::Wait
    } else if recovery_needed && now_ms < retry_after_ms {
        RecoveryAction::Defer
    } else if recovery_needed {
        RecoveryAction::Recover
    } else {
        RecoveryAction::Admit
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DebounceResult {
    Pending,
    Stable(u32),
    TimedOut,
}

/// Pure state machine for Linux's normal SATA link debounce policy. A DET
/// value must remain unchanged for `stable_ms`; DET=1 (device present but no
/// communication) is deliberately held until the full deadline so slow PHY
/// negotiation is not mistaken for a stable offline link.
pub(crate) struct SataDebounce {
    started_ms: u64,
    last_change_ms: u64,
    last_det: u32,
    stable_ms: u64,
    timeout_ms: u64,
}

impl SataDebounce {
    pub(crate) const fn new(
        started_ms: u64,
        initial_det: u32,
        stable_ms: u64,
        timeout_ms: u64,
    ) -> Self {
        Self {
            started_ms,
            last_change_ms: started_ms,
            last_det: initial_det,
            stable_ms,
            timeout_ms,
        }
    }

    pub(crate) fn observe(&mut self, now_ms: u64, det: u32) -> DebounceResult {
        let elapsed = now_ms.saturating_sub(self.started_ms);
        if det != self.last_det {
            self.last_det = det;
            self.last_change_ms = now_ms;
            return if elapsed >= self.timeout_ms {
                DebounceResult::TimedOut
            } else {
                DebounceResult::Pending
            };
        }

        if det == 1 && elapsed < self.timeout_ms {
            return DebounceResult::Pending;
        }
        if now_ms.saturating_sub(self.last_change_ms) >= self.stable_ms {
            return DebounceResult::Stable(det);
        }
        if elapsed >= self.timeout_ms {
            DebounceResult::TimedOut
        } else {
            DebounceResult::Pending
        }
    }
}

pub(crate) fn transfer_in_range(
    block_id: u64,
    byte_len: usize,
    block_size: usize,
    block_count: u64,
) -> bool {
    if block_size == 0 || byte_len % block_size != 0 {
        return false;
    }
    let blocks = (byte_len / block_size) as u64;
    block_id
        .checked_add(blocks)
        .is_some_and(|end| end <= block_count)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, atomic::AtomicBool},
        thread,
    };

    use super::*;

    #[test]
    fn publishing_cannot_clear_a_reused_slots_issued_bit() {
        let queue = Arc::new(CompletionQueue::new(1));
        let old = queue.try_reserve().unwrap();
        queue.mark_issued(old);
        queue.publish_pause.store(1, Ordering::Release);

        let publisher = Arc::clone(&queue);
        let publishing = thread::spawn(move || publisher.publish(1, false));
        while queue.publish_pause.load(Ordering::Acquire) != 2 {
            thread::yield_now();
        }

        assert_eq!(queue.take(old), Some(true));
        let current = queue.try_reserve().unwrap();
        queue.mark_issued(current);
        queue.publish_pause.store(3, Ordering::Release);
        assert_eq!(publishing.join().unwrap(), 1);

        assert_eq!(queue.issued(), 1);
        queue.cancel(current);
    }

    #[test]
    fn a_late_success_publisher_cannot_overwrite_a_claimed_error() {
        let queue = Arc::new(CompletionQueue::new(1));
        let token = queue.try_reserve().unwrap();
        queue.mark_issued(token);
        queue.publish_claim_pause.store(1, Ordering::Release);

        let publisher = Arc::clone(&queue);
        let failing = thread::spawn(move || publisher.publish(1, true));
        while queue.publish_claim_pause.load(Ordering::Acquire) != 2 {
            thread::yield_now();
        }

        assert_eq!(queue.publish(1, false), 0);
        queue.publish_claim_pause.store(3, Ordering::Release);
        assert_eq!(failing.join().unwrap(), 1);
        assert_eq!(queue.take(token), Some(false));
    }

    #[test]
    fn a_success_claim_is_revalidated_against_a_concurrent_fatal_irq() {
        let queue = Arc::new(CompletionQueue::new(1));
        let recovery_needed = Arc::new(AtomicBool::new(false));
        let token = queue.try_reserve().unwrap();
        queue.mark_issued(token);
        queue.publish_claim_pause.store(1, Ordering::Release);

        let publisher = Arc::clone(&queue);
        let recovery = Arc::clone(&recovery_needed);
        let completing = thread::spawn(move || {
            publisher.publish_checked(1, false, || recovery.load(Ordering::Acquire))
        });
        while queue.publish_claim_pause.load(Ordering::Acquire) != 2 {
            thread::yield_now();
        }

        recovery_needed.store(true, Ordering::Release);
        assert_eq!(queue.publish(1, true), 0);
        queue.publish_claim_pause.store(3, Ordering::Release);
        assert_eq!(completing.join().unwrap(), 1);
        assert_eq!(queue.take(token), Some(false));
    }

    #[test]
    fn supports_out_of_order_completion() {
        let queue = CompletionQueue::new(4);
        let first = queue.try_reserve().unwrap();
        let second = queue.try_reserve().unwrap();
        queue.mark_issued(first);
        queue.mark_issued(second);

        queue.publish(1_u32 << second.slot, false);
        assert_eq!(queue.take(second), Some(true));
        assert_eq!(queue.take(first), None);
        queue.publish(1_u32 << first.slot, false);
        assert_eq!(queue.take(first), Some(true));
    }

    #[test]
    fn rejects_stale_generation() {
        let queue = CompletionQueue::new(1);
        let old = queue.try_reserve().unwrap();
        queue.mark_issued(old);
        queue.publish(1, false);
        assert_eq!(queue.take(old), Some(true));

        let current = queue.try_reserve().unwrap();
        assert_ne!(old.generation, current.generation);
        queue.mark_issued(current);
        queue.publish(1, false);
        assert_eq!(queue.take(old), None);
        assert_eq!(queue.take(current), Some(true));
    }

    #[test]
    fn reports_errors_and_capacity() {
        let queue = CompletionQueue::new(2);
        let first = queue.try_reserve().unwrap();
        let second = queue.try_reserve().unwrap();
        queue.mark_issued(first);
        queue.mark_issued(second);
        assert!(queue.try_reserve().is_none());

        queue.publish(queue.issued(), true);
        assert_eq!(queue.take(first), Some(false));
        assert_eq!(queue.take(second), Some(false));
        assert!(queue.try_reserve().is_some());
    }

    #[test]
    fn ignores_a_reserved_but_unissued_slot() {
        let queue = CompletionQueue::new(1);
        let token = queue.try_reserve().unwrap();

        assert_eq!(queue.publish(1, false), 0);
        assert_eq!(queue.take(token), None);
        queue.mark_issued(token);
        assert_eq!(queue.publish(1, false), 1);
        assert_eq!(queue.take(token), Some(true));
    }

    #[test]
    fn fatal_interrupt_completes_every_issued_slot() {
        assert_eq!(completed_commands(0b1011, 0b1010, false), 0b0001);
        assert_eq!(completed_commands(0b1011, 0b1010, true), 0b1011);
        assert_eq!(completed_commands(0b0011, 0, false), 0b0011);
    }

    #[test]
    fn non_ncq_admission_is_fifo() {
        let admission = NonNcqAdmission::new();
        let first = admission.reserve();
        let second = admission.reserve();
        let third = admission.reserve();
        assert!(admission.is_turn(first));
        assert!(!admission.is_turn(second));
        admission.release(first);
        assert!(admission.is_turn(second));
        assert!(!admission.is_turn(third));
        admission.release(second);
        assert!(admission.is_turn(third));
        admission.release(third);
    }

    #[test]
    fn unpublished_state_can_be_reinitialized_after_relocation() {
        let mut admission = NonNcqAdmission::new();
        let first = admission.reserve();
        let _queued = admission.reserve();
        admission.release(first);
        admission.reinitialize_unpublished();
        let reset = admission.reserve();
        assert!(admission.is_turn(reset));

        let mut queue = CompletionQueue::new(2);
        let stale = queue.try_reserve().unwrap();
        queue.mark_issued(stale);
        queue.reinitialize_unpublished();
        assert_eq!(queue.issued(), 0);
        let current = queue.try_reserve().unwrap();
        assert_eq!(current.slot, 0);
        assert_eq!(current.generation, 1);
    }

    #[test]
    fn classifies_fatal_port_interrupts() {
        assert!(port_interrupt_failed(1 << 30));
        assert!(port_interrupt_failed(1 << 4));
        assert!(!port_interrupt_failed(1));
    }

    #[test]
    fn pending_recovery_prevents_a_cleared_fatal_irq_from_becoming_success() {
        assert!(command_completion_failed(true, 0, false));
        assert!(command_completion_failed(false, 1 << 30, false));
        assert!(command_completion_failed(false, 0, true));
        assert!(!command_completion_failed(false, 0, false));
    }

    #[test]
    fn idle_port_error_is_recovered_before_next_command_admission() {
        assert_eq!(
            recovery_action(false, false, true, 1_000, 0),
            RecoveryAction::Recover
        );
        assert_eq!(
            recovery_action(false, true, true, 1_000, 0),
            RecoveryAction::Wait
        );
        assert_eq!(
            recovery_action(true, false, true, 1_000, 0),
            RecoveryAction::Fail
        );
        assert_eq!(
            recovery_action(false, false, false, 1_000, 0),
            RecoveryAction::Admit
        );
    }

    #[test]
    fn retryable_recovery_waits_for_retry_deadline() {
        assert_eq!(
            recovery_action(false, false, true, 59_999, 60_000),
            RecoveryAction::Defer
        );
        assert_eq!(
            recovery_action(false, false, true, 60_000, 60_000),
            RecoveryAction::Recover
        );
    }

    #[test]
    fn sata_link_requires_a_continuously_stable_det_value() {
        let mut debounce = SataDebounce::new(0, 1, 100, 2_000);
        assert_eq!(debounce.observe(50, 3), DebounceResult::Pending);
        assert_eq!(debounce.observe(149, 3), DebounceResult::Pending);
        assert_eq!(debounce.observe(150, 3), DebounceResult::Stable(3));

        let mut unstable = SataDebounce::new(0, 1, 100, 2_000);
        assert_eq!(unstable.observe(1_950, 3), DebounceResult::Pending);
        assert_eq!(unstable.observe(2_010, 0), DebounceResult::TimedOut);
    }

    #[test]
    fn sata_det_one_is_not_accepted_before_the_full_timeout() {
        let mut debounce = SataDebounce::new(0, 1, 100, 2_000);
        assert_eq!(debounce.observe(1_999, 1), DebounceResult::Pending);
        assert_eq!(debounce.observe(2_000, 1), DebounceResult::Stable(1));
    }

    #[test]
    fn validates_block_alignment_and_disk_bounds() {
        assert!(transfer_in_range(0, 512, 512, 8));
        assert!(transfer_in_range(7, 512, 512, 8));
        assert!(transfer_in_range(8, 0, 512, 8));
        assert!(!transfer_in_range(7, 1024, 512, 8));
        assert!(!transfer_in_range(0, 513, 512, 8));
        assert!(!transfer_in_range(u64::MAX, 512, 512, u64::MAX));
    }
}
