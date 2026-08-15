use core::{future::poll_fn, task::Poll};

use axerrno::{AxError, AxResult};
use axpoll::{IoEvents, Pollable};

/// A helper to wrap a synchronous non-blocking I/O function into an
/// asynchronous function.
///
/// # Arguments
///
/// * `pollable`: The pollable object to register for I/O events.
/// * `events`: The I/O events to wait for.
/// * `non_blocking`: If true, the function will return `AxError::WouldBlock`
///   immediately when the I/O operation would block.
/// * `f`: The synchronous non-blocking I/O function to be wrapped. It should
///   return `AxError::WouldBlock` when the operation would block.
pub async fn poll_io<P: Pollable, F: FnMut() -> AxResult<T>, T>(
    pollable: &P,
    events: IoEvents,
    non_blocking: bool,
    mut f: F,
) -> AxResult<T> {
    super::interruptible(poll_fn(move |cx| match f() {
        Ok(value) => Poll::Ready(Ok(value)),
        Err(AxError::WouldBlock) => {
            if non_blocking {
                return Poll::Ready(Err(AxError::WouldBlock));
            }
            pollable.register(cx, events);
            match f() {
                Ok(value) => Poll::Ready(Ok(value)),
                Err(AxError::WouldBlock) => Poll::Pending,
                Err(e) => Poll::Ready(Err(e)),
            }
        }
        Err(e) => Poll::Ready(Err(e)),
    }))
    .await?
}

#[cfg(feature = "irq")]
/// Registers a waker for the given IRQ number.
pub fn register_irq_waker(irq: usize, waker: &core::task::Waker) {
    register_irq_waker_inner(irq, waker, false);
}

#[cfg(feature = "irq")]
/// Registers a waker for an IRQ whose device driver already owns the handler.
pub fn register_driver_irq_waker(irq: usize, waker: &core::task::Waker) {
    register_irq_waker_inner(irq, waker, true);
}

#[cfg(feature = "irq")]
fn register_irq_waker_inner(
    irq: usize,
    waker: &core::task::Waker,
    handler_already_registered: bool,
) {
    use alloc::collections::{BTreeMap, btree_map::Entry};

    use axpoll::PollSet;
    use kspin::SpinNoIrq;

    static POLL_IRQ: SpinNoIrq<BTreeMap<usize, PollSet>> = SpinNoIrq::new(BTreeMap::new());

    fn irq_event_marker() {}

    fn irq_hook(irq: usize) {
        // Event-waker IRQs are one-shot.  Mask before waking the consumer so a
        // level-triggered UART cannot retrigger forever while its FIFO still
        // contains the byte that woke us.  The next poll registration rearms
        // the controller after the device has been drained.
        let registered = POLL_IRQ.lock().contains_key(&irq);
        if !registered {
            return;
        }
        axhal::irq::set_enable(irq, false);
        if let Some(s) = POLL_IRQ.lock().get(&irq) {
            s.wake();
        }
    }

    let first_registration = {
        let mut poll_irq = POLL_IRQ.lock();
        let first = !poll_irq.contains_key(&irq);
        match poll_irq.entry(irq) {
            Entry::Vacant(entry) => entry.insert(PollSet::new()),
            Entry::Occupied(entry) => entry.into_mut(),
        }
        .register(waker);
        first
    };

    if first_registration {
        axhal::irq::register_irq_hook(irq_hook);
        // Give hook-only sources (such as the LS2K1000 UART) a marker handler
        // so the platform does not classify every byte as an unhandled IRQ.
        // A real driver may already own the source, in which case registration
        // correctly fails and only the shared post-IRQ hook is used.
        if !handler_already_registered && axhal::irq::register(irq, irq_event_marker) {
            return;
        }
    }
    // This is also required for the occupied path: irq_hook masked the source
    // after its previous event, so merely replacing the waker is insufficient.
    axhal::irq::set_enable(irq, true);
}
