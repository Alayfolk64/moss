use core::task::Waker;

use axplat::{console::ConsoleIf, console_rx::ConsoleRxBuffer};
use kspin::SpinNoIrq;

use crate::fdt::{UartResource, resources, uncached_addr};

const UART_RBR_THR: usize = 0;
const UART_IER: usize = 1;
const UART_FCR: usize = 2;
const UART_LSR: usize = 5;
const UART_IER_RECEIVED_DATA_AVAILABLE: u8 = 1;
const UART_FCR_ENABLE_AND_CLEAR: u8 = 0x07;
const UART_LSR_DATA_READY: u32 = 1;
const UART_LSR_OVERRUN_ERROR: u32 = 1 << 1;
const UART_LSR_THR_EMPTY: u32 = 1 << 5;
const RX_BUFFER_SIZE: usize = 8 * 1024;
const RX_DRAIN_LIMIT: usize = 256;

struct Ns16550 {
    base: usize,
    reg_shift: usize,
    reg_io_width: usize,
}

unsafe impl Send for Ns16550 {}
unsafe impl Sync for Ns16550 {}

impl Ns16550 {
    const fn uninit() -> Self {
        Self {
            base: 0,
            reg_shift: 0,
            reg_io_width: 1,
        }
    }

    fn new(resource: UartResource) -> Self {
        let uart = Self {
            base: uncached_addr(resource.paddr),
            reg_shift: resource.reg_shift,
            reg_io_width: resource.reg_io_width,
        };
        // Preserve the firmware baud divisor; the board DT commonly describes
        // the source through a clock controller rather than clock-frequency.
        uart.write(UART_IER, 0);
        uart.write(UART_FCR, UART_FCR_ENABLE_AND_CLEAR);
        uart
    }

    fn address(&self, register: usize) -> usize {
        self.base + (register << self.reg_shift)
    }

    fn read(&self, register: usize) -> u32 {
        unsafe {
            match self.reg_io_width {
                1 => (self.address(register) as *const u8).read_volatile() as u32,
                4 => (self.address(register) as *const u32).read_volatile(),
                _ => unreachable!(),
            }
        }
    }

    fn write(&self, register: usize, value: u8) {
        unsafe {
            match self.reg_io_width {
                1 => (self.address(register) as *mut u8).write_volatile(value),
                4 => (self.address(register) as *mut u32).write_volatile(value as u32),
                _ => unreachable!(),
            }
        }
    }

    fn put(&self, value: u8) {
        while self.read(UART_LSR) & UART_LSR_THR_EMPTY == 0 {
            core::hint::spin_loop();
        }
        self.write(UART_RBR_THR, value);
    }
}

// Keep the lock at its final address and initialize only the aligned UART
// payload in place.  Moving `SpinNoIrq<Ns16550>` through `LazyInit::init_once`
// also copies the lock's seven padding bytes; the pinned LoongArch backend
// widened that padding copy into unaligned word loads/stores on LA264.
static UART: SpinNoIrq<Ns16550> = SpinNoIrq::new(Ns16550::uninit());
static RX: SpinNoIrq<ConsoleRxBuffer<RX_BUFFER_SIZE>> = SpinNoIrq::new(ConsoleRxBuffer::new());

/// Drains the bounded NS16550 hardware FIFO into the larger software ring.
///
/// The UART lock is released before the RX lock is acquired, and the caller
/// wakes the tty outside both locks. This keeps the IRQ path non-recursive.
fn drain_hardware_rx() -> Option<Waker> {
    let mut batch = [0; RX_DRAIN_LIMIT];
    let (received, hardware_overruns) = {
        let uart = UART.lock();
        let mut received = 0;
        let mut hardware_overruns = 0usize;
        while received < batch.len() {
            let status = uart.read(UART_LSR);
            if status & UART_LSR_OVERRUN_ERROR != 0 {
                hardware_overruns = hardware_overruns.saturating_add(1);
            }
            if status & UART_LSR_DATA_READY == 0 {
                break;
            }
            batch[received] = uart.read(UART_RBR_THR) as u8;
            received += 1;
        }
        (received, hardware_overruns)
    };

    let mut rx = RX.lock();
    rx.note_hardware_overruns(hardware_overruns);
    if received == 0 {
        return None;
    }
    rx.push_slice(&batch[..received]);
    rx.pending_waker()
}

#[cfg(feature = "irq")]
fn handle_uart_interrupt() {
    if let Some(waker) = drain_hardware_rx() {
        waker.wake();
    }
}

pub(crate) fn init_early() {
    let resource = resources().uart;
    *UART.lock() = Ns16550::new(resource);
    info!(
        "LS2K1000 console from DTB: paddr={:#x}, reg_shift={}, reg_io_width={}, clock_hz={:?}, \
         irq={:?}",
        resource.paddr, resource.reg_shift, resource.reg_io_width, resource.clock_hz, resource.irq,
    );
}

#[cfg(feature = "irq")]
pub(crate) fn init_irq() {
    let irq = resources()
        .uart
        .irq
        .expect("LS2K1000 UART is missing its DT interrupt");
    assert!(
        axplat::irq::register(irq, handle_uart_interrupt),
        "failed to register LS2K1000 UART IRQ {irq}"
    );

    // The controller and handler are live before the NS16550 can assert its
    // level interrupt, so queued firmware-era input cannot hit an empty slot.
    let uart = UART.lock();
    let ier = uart.read(UART_IER) as u8;
    uart.write(UART_IER, ier | UART_IER_RECEIVED_DATA_AVAILABLE);
}

struct ConsoleIfImpl;

#[impl_plat_interface]
impl ConsoleIf for ConsoleIfImpl {
    fn write_bytes(bytes: &[u8]) {
        // Higher layers serialize normal writes. Releasing the IRQ-safe UART
        // lock after each character bounds RX IRQ blocking to one character
        // time at 115200 baud instead of the duration of the whole buffer.
        for &byte in bytes {
            if byte == b'\n' {
                UART.lock().put(b'\r');
            }
            UART.lock().put(byte);
        }
    }

    fn read_bytes(bytes: &mut [u8]) -> usize {
        // Poll once as well as servicing IRQs. This covers the short interval
        // before late IRQ initialization and makes diagnostics deterministic.
        drop(drain_hardware_rx());
        RX.lock().pop_slice(bytes)
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> {
        resources().uart.irq
    }

    #[cfg(feature = "irq")]
    fn enable_rx_interrupt() {}

    #[cfg(feature = "irq")]
    fn register_rx_waker(waker: &Waker) -> bool {
        let pending = RX.lock().register_waker(waker);
        if pending {
            waker.wake_by_ref();
        }
        true
    }
}
