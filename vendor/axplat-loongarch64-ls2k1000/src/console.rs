use axplat::console::ConsoleIf;
use kspin::SpinNoIrq;

use crate::fdt::{UartResource, resources, uncached_addr};

const UART_RBR_THR: usize = 0;
const UART_IER: usize = 1;
const UART_LSR: usize = 5;
const UART_IER_RECEIVED_DATA_AVAILABLE: u8 = 1;
const UART_LSR_DATA_READY: u32 = 1;
const UART_LSR_THR_EMPTY: u32 = 1 << 5;

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

pub(crate) fn init_early() {
    let resource = resources().uart;
    *UART.lock() = Ns16550::new(resource);
    info!(
        "LS2K1000 console from DTB: paddr={:#x}, reg_shift={}, reg_io_width={}, clock_hz={:?}, \
         irq={:?}",
        resource.paddr, resource.reg_shift, resource.reg_io_width, resource.clock_hz, resource.irq,
    );
}

struct ConsoleIfImpl;

#[impl_plat_interface]
impl ConsoleIf for ConsoleIfImpl {
    fn write_bytes(bytes: &[u8]) {
        let uart = UART.lock();
        for &byte in bytes {
            if byte == b'\n' {
                uart.put(b'\r');
            }
            uart.put(byte);
        }
    }

    fn read_bytes(bytes: &mut [u8]) -> usize {
        let uart = UART.lock();
        for (index, byte) in bytes.iter_mut().enumerate() {
            if uart.read(UART_LSR) & UART_LSR_DATA_READY == 0 {
                return index;
            }
            *byte = uart.read(UART_RBR_THR) as u8;
        }
        bytes.len()
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> {
        resources().uart.irq
    }

    #[cfg(feature = "irq")]
    fn enable_rx_interrupt() {
        let changed = {
            let uart = UART.lock();
            let ier = uart.read(UART_IER) as u8;
            if ier & UART_IER_RECEIVED_DATA_AVAILABLE == 0 {
                uart.write(UART_IER, ier | UART_IER_RECEIVED_DATA_AVAILABLE);
                true
            } else {
                false
            }
        };
        // Never print through ConsoleIf while holding UART: the print path
        // takes the same lock and would deadlock before the first RX IRQ.
        if changed {
            warn!("LS2K1000 console RX interrupt enabled");
        }
    }
}
