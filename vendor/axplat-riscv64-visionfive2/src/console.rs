use axplat::{
    console::ConsoleIf,
    mem::{pa, phys_to_virt},
};
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use uart_16550::MmioSerialPort;

use crate::config::{
    devices::{UART_IRQ, UART_PADDR},
    plat::PHYS_VIRT_OFFSET,
};

const UART_REGISTER_STRIDE: usize = 4;
const UART_IER_OFFSET: usize = UART_REGISTER_STRIDE;
const UART_IER_RX_AVAILABLE: u32 = 1;
const RX_BUFFER_SIZE: usize = 1024;
const RX_DRAIN_LIMIT: usize = 256;

struct ConsoleUart {
    port: MmioSerialPort,
    rx_buffer: [u8; RX_BUFFER_SIZE],
    rx_head: usize,
    rx_len: usize,
    rx_dropped: usize,
}

impl ConsoleUart {
    fn new(base: usize) -> Self {
        Self {
            port: unsafe { MmioSerialPort::new_with_stride(base, UART_REGISTER_STRIDE) },
            rx_buffer: [0; RX_BUFFER_SIZE],
            rx_head: 0,
            rx_len: 0,
            rx_dropped: 0,
        }
    }

    fn push_rx(&mut self, byte: u8) {
        if self.rx_len == RX_BUFFER_SIZE {
            self.rx_dropped = self.rx_dropped.saturating_add(1);
            return;
        }
        let tail = (self.rx_head + self.rx_len) % RX_BUFFER_SIZE;
        self.rx_buffer[tail] = byte;
        self.rx_len += 1;
    }

    fn pop_rx(&mut self) -> Option<u8> {
        if self.rx_len == 0 {
            return None;
        }
        let byte = self.rx_buffer[self.rx_head];
        self.rx_head = (self.rx_head + 1) % RX_BUFFER_SIZE;
        self.rx_len -= 1;
        Some(byte)
    }

    fn drain_hardware_rx(&mut self) {
        for _ in 0..RX_DRAIN_LIMIT {
            let Ok(byte) = self.port.try_receive() else {
                break;
            };
            self.push_rx(byte);
        }
    }
}

static UART: LazyInit<SpinNoIrq<ConsoleUart>> = LazyInit::new();

fn set_rx_interrupt(enabled: bool) {
    let value = if enabled { UART_IER_RX_AVAILABLE } else { 0 };
    unsafe {
        ((PHYS_VIRT_OFFSET + UART_PADDR + UART_IER_OFFSET) as *mut u32).write_volatile(value);
    }
}

fn handle_uart_interrupt() {
    UART.lock().drain_hardware_rx();
}

pub(crate) fn init_early() {
    UART.init_once({
        // JH7110 exposes the 8250 registers at a four-byte stride. U-Boot has
        // already configured the clock and baud rate, so do not reinitialize it.
        let uart = ConsoleUart::new(phys_to_virt(pa!(UART_PADDR)).as_usize());
        SpinNoIrq::new(uart)
    });
    // Firmware owns the UART before Moss starts. Mask its device-side source
    // until the PLIC handler has been installed during late platform init.
    set_rx_interrupt(false);
}

#[cfg(feature = "irq")]
pub(crate) fn init_irq() {
    assert!(
        axplat::irq::register(UART_IRQ, handle_uart_interrupt),
        "failed to register VF2 UART IRQ {UART_IRQ}"
    );
    set_rx_interrupt(true);
}

struct ConsoleIfImpl;

#[impl_plat_interface]
impl ConsoleIf for ConsoleIfImpl {
    fn write_bytes(bytes: &[u8]) {
        // Keep one userspace/kernel write contiguous on SMP. Locking once per
        // byte lets writers on different U74 cores splice their text together.
        let mut uart = UART.lock();
        for &byte in bytes {
            if byte == b'\n' {
                uart.port.send_raw(b'\r');
            }
            uart.port.send_raw(byte);
        }
    }

    fn read_bytes(bytes: &mut [u8]) -> usize {
        let mut uart = UART.lock();
        for (index, byte) in bytes.iter_mut().enumerate() {
            if let Some(received) = uart.pop_rx() {
                *byte = received;
                continue;
            }
            match uart.port.try_receive() {
                Ok(received) => *byte = received,
                Err(_) => return index,
            }
        }
        bytes.len()
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> {
        Some(UART_IRQ)
    }
}
