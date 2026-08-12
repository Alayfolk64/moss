//! Allocation-free receive buffering shared by interrupt-driven consoles.

use core::task::Waker;

/// A fixed-size byte ring with a persistent reader wake target.
///
/// Platform interrupt handlers drain the small hardware FIFO into this ring.
/// Keeping the waker here lets the device interrupt remain enabled after one
/// wake-up instead of relying on the generic one-shot IRQ masking policy.
pub struct ConsoleRxBuffer<const N: usize> {
    bytes: [u8; N],
    head: usize,
    len: usize,
    dropped: usize,
    hardware_overruns: usize,
    waker: Option<Waker>,
}

impl<const N: usize> ConsoleRxBuffer<N> {
    /// Creates an empty receive buffer.
    pub const fn new() -> Self {
        assert!(N > 0, "console RX buffer must not be empty");
        Self {
            bytes: [0; N],
            head: 0,
            len: 0,
            dropped: 0,
            hardware_overruns: 0,
            waker: None,
        }
    }

    /// Appends as many bytes as fit and returns the accepted byte count.
    pub fn push_slice(&mut self, input: &[u8]) -> usize {
        let accepted = input.len().min(N - self.len);
        for &byte in &input[..accepted] {
            let tail = (self.head + self.len) % N;
            self.bytes[tail] = byte;
            self.len += 1;
        }
        self.dropped = self
            .dropped
            .saturating_add(input.len().saturating_sub(accepted));
        accepted
    }

    /// Removes bytes in arrival order and returns the copied byte count.
    pub fn pop_slice(&mut self, output: &mut [u8]) -> usize {
        let copied = output.len().min(self.len);
        for byte in &mut output[..copied] {
            *byte = self.bytes[self.head];
            self.head = (self.head + 1) % N;
            self.len -= 1;
        }
        copied
    }

    /// Records receive bytes that hardware reported lost before software read them.
    pub fn note_hardware_overruns(&mut self, count: usize) {
        self.hardware_overruns = self.hardware_overruns.saturating_add(count);
    }

    /// Installs the current receive waiter and reports whether data is pending.
    pub fn register_waker(&mut self, waker: &Waker) -> bool {
        if !self
            .waker
            .as_ref()
            .is_some_and(|registered| registered.will_wake(waker))
        {
            self.waker = Some(waker.clone());
        }
        self.len != 0
    }

    /// Clones the registered wake target when unread data exists.
    pub fn pending_waker(&self) -> Option<Waker> {
        (self.len != 0).then(|| self.waker.clone()).flatten()
    }

    /// Returns the number of bytes currently buffered.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the buffer is empty.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of bytes dropped because the software ring was full.
    pub const fn dropped(&self) -> usize {
        self.dropped
    }

    /// Returns the number of overruns reported by the UART hardware.
    pub const fn hardware_overruns(&self) -> usize {
        self.hardware_overruns
    }
}

impl<const N: usize> Default for ConsoleRxBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Wake, Waker},
    };

    use super::ConsoleRxBuffer;

    struct Counter(AtomicUsize);

    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn wraps_without_reordering_bytes() {
        let mut rx = ConsoleRxBuffer::<4>::new();
        assert_eq!(rx.push_slice(b"abc"), 3);
        let mut first = [0; 2];
        assert_eq!(rx.pop_slice(&mut first), 2);
        assert_eq!(&first, b"ab");
        assert_eq!(rx.push_slice(b"de"), 2);
        let mut rest = [0; 3];
        assert_eq!(rx.pop_slice(&mut rest), 3);
        assert_eq!(&rest, b"cde");
        assert!(rx.is_empty());
    }

    #[test]
    fn counts_full_ring_and_hardware_drops_safely() {
        let mut rx = ConsoleRxBuffer::<2>::new();
        assert_eq!(rx.push_slice(b"abcde"), 2);
        rx.note_hardware_overruns(3);
        assert_eq!(rx.len(), 2);
        assert_eq!(rx.dropped(), 3);
        assert_eq!(rx.hardware_overruns(), 3);
    }

    #[test]
    fn publishes_waker_for_pending_input() {
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let mut rx = ConsoleRxBuffer::<4>::new();
        assert!(!rx.register_waker(&waker));
        rx.push_slice(b"x");
        rx.pending_waker().unwrap().wake();
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert!(rx.register_waker(&waker));
    }
}
