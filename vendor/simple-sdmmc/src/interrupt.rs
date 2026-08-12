use core::{
    ptr::NonNull,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::Duration,
};

use volatile::VolatilePtr;

use crate::{
    idmac,
    regs::{RegisterBlock, RegisterBlockVolatileFieldAccess},
};

/// Synthetic event recorded when IDMAC completes a receive or transmit.
pub(crate) const IDMAC_COMPLETE: u32 = 1 << 30;
/// Synthetic event recorded when IDMAC reports an abnormal condition.
pub(crate) const IDMAC_ERROR: u32 = 1 << 31;

const GENERATION_SHIFT: u32 = 32;
const STATUS_MASK: u64 = u32::MAX as u64;

/// Platform hooks used by the interrupt-driven SD/MMC path.
pub trait InterruptWait: Sync {
    /// Opens a new interrupt generation and discards older notifications.
    fn prepare(&self);
    /// Waits for a latched controller event. Returns `false` on timeout.
    fn wait(&self, timeout: Duration) -> bool;
    /// Atomically consumes the status latched by the interrupt handler.
    fn take_status(&self) -> u32;
    /// Closes the current interrupt generation.
    fn finish(&self);
    /// Unmasks the controller interrupt output.
    fn rearm(&self);
    /// Returns a monotonic timestamp in milliseconds.
    fn current_ms(&self) -> u64;
}

/// Generation-tagged storage for destructive SDMMC interrupt status.
pub struct InterruptMailbox {
    value: AtomicU64,
    next_generation: AtomicU32,
}

impl InterruptMailbox {
    /// Creates an inactive mailbox.
    pub const fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
            next_generation: AtomicU32::new(0),
        }
    }

    /// Opens and returns a new nonzero request generation.
    pub fn begin(&self) -> u32 {
        let mut generation = self
            .next_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if generation == 0 {
            generation = self
                .next_generation
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
        }
        self.value.store(pack(generation, 0), Ordering::Release);
        generation
    }

    /// Closes the current request so late events cannot be retained.
    pub fn finish(&self) {
        self.value.store(0, Ordering::Release);
    }

    /// Returns the active request generation, or zero if inactive.
    pub fn generation(&self) -> u32 {
        unpack_generation(self.value.load(Ordering::Acquire))
    }

    /// Adds events only when the captured generation is still active.
    pub fn cache_if_current(&self, generation: u32, events: u32) {
        if generation == 0 || events == 0 {
            return;
        }
        let mut current = self.value.load(Ordering::Acquire);
        loop {
            if unpack_generation(current) != generation {
                return;
            }
            let next = pack(generation, unpack_status(current) | events);
            match self.value.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    /// Returns whether the current request has cached events.
    pub fn pending(&self) -> bool {
        unpack_status(self.value.load(Ordering::Acquire)) != 0
    }

    /// Atomically consumes every cached event for the current request.
    pub fn take(&self) -> u32 {
        let mut current = self.value.load(Ordering::Acquire);
        loop {
            let events = unpack_status(current);
            if events == 0 {
                return 0;
            }
            let next = pack(unpack_generation(current), 0);
            match self.value.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return events,
                Err(observed) => current = observed,
            }
        }
    }
}

impl Default for InterruptMailbox {
    fn default() -> Self {
        Self::new()
    }
}

const fn pack(generation: u32, status: u32) -> u64 {
    ((generation as u64) << GENERATION_SHIFT) | status as u64
}

const fn unpack_generation(value: u64) -> u32 {
    (value >> GENERATION_SHIFT) as u32
}

const fn unpack_status(value: u64) -> u32 {
    (value & STATUS_MASK) as u32
}

/// Acknowledges controller and IDMAC status and returns a stable event set.
///
/// The platform must retain the result before waking the request task because
/// both hardware registers are write-one-to-clear.
///
/// # Safety
///
/// `base` must point to the SD/MMC register block exclusively owned by this driver.
pub unsafe fn acknowledge_interrupt(base: usize) -> u32 {
    let regs = unsafe { VolatilePtr::new(NonNull::new_unchecked(base as *mut RegisterBlock)) };
    let controller = regs.mintsts().read().into_bits();
    if controller != 0 {
        regs.rintsts().write(controller.into());
    }

    let idmac_status = regs.idsts().read();
    let idmac_ack = idmac_status & idmac::INT_CLEAR;
    if idmac_ack != 0 {
        regs.idsts().write(idmac_ack);
    }
    let enabled_idmac = idmac_status & regs.idinten().read();
    let mut events = controller;
    if enabled_idmac & (idmac::INT_TI | idmac::INT_RI) != 0 {
        events |= IDMAC_COMPLETE;
    }
    if enabled_idmac & idmac::INT_ERROR != 0 {
        events |= IDMAC_ERROR;
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separately_arriving_events_are_not_lost() {
        let mailbox = InterruptMailbox::new();
        let generation = mailbox.begin();
        mailbox.cache_if_current(generation, 1 << 2);
        mailbox.cache_if_current(generation, IDMAC_COMPLETE);
        assert_eq!(mailbox.take(), (1 << 2) | IDMAC_COMPLETE);
        assert!(!mailbox.pending());
    }

    #[test]
    fn late_event_from_previous_request_is_discarded() {
        let mailbox = InterruptMailbox::new();
        let old = mailbox.begin();
        let current = mailbox.begin();
        mailbox.cache_if_current(old, IDMAC_ERROR);
        mailbox.cache_if_current(current, IDMAC_COMPLETE);
        assert_eq!(mailbox.take(), IDMAC_COMPLETE);
    }

    #[test]
    fn finishing_request_discards_late_event() {
        let mailbox = InterruptMailbox::new();
        let generation = mailbox.begin();
        mailbox.finish();
        mailbox.cache_if_current(generation, IDMAC_COMPLETE);
        assert_eq!(mailbox.generation(), 0);
        assert_eq!(mailbox.take(), 0);
    }
}
