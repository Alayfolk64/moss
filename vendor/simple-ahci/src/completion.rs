//! Lock-free AHCI command completion state shared by submitters and IRQ code.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

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
}

/// Serializes non-NCQ ATA commands while allowing callers to sleep outside
/// the admission state. Linux libata enforces the same one-command rule.
pub(crate) struct NonNcqAdmission {
    active: AtomicBool,
}

impl NonNcqAdmission {
    pub(crate) const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
        }
    }

    pub(crate) fn try_acquire(&self) -> bool {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn release(&self) {
        let was_active = self.active.swap(false, Ordering::AcqRel);
        debug_assert!(was_active);
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
        let published = mask & self.issued();
        for slot in 0..self.capacity {
            if published & (1_u32 << slot) != 0 {
                self.slots[slot].state.store(
                    if failed { SLOT_ERROR } else { SLOT_COMPLETE },
                    Ordering::Release,
                );
            }
        }
        self.issued.fetch_and(!published, Ordering::AcqRel);
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

pub(crate) fn port_interrupt_failed(status_bits: u32) -> bool {
    status_bits & PORT_IRQ_FATAL_MASK != 0
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
    use super::*;

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
    fn non_ncq_admission_allows_only_one_active_command() {
        let admission = NonNcqAdmission::new();
        assert!(admission.try_acquire());
        assert!(!admission.try_acquire());
        admission.release();
        assert!(admission.try_acquire());
    }

    #[test]
    fn classifies_fatal_port_interrupts() {
        assert!(port_interrupt_failed(1 << 30));
        assert!(port_interrupt_failed(1 << 4));
        assert!(!port_interrupt_failed(1));
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
