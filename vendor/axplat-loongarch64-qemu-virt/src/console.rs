use axplat::{
    console::ConsoleIf,
    mem::{pa, phys_to_virt},
};
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use uart_16550::MmioSerialPort;

use crate::config::devices::UART_PADDR;

const UART_FIFO_CONTROL_OFFSET: usize = 2;
const UART_FIFO_ENABLE_CLEAR_TRIGGER_ONE: u8 = 0x07;

static UART: LazyInit<SpinNoIrq<MmioSerialPort>> = LazyInit::new();

pub(crate) fn init_early() {
    UART.init_once({
        let uart_base = phys_to_virt(pa!(UART_PADDR)).as_usize();
        let mut uart = unsafe { MmioSerialPort::new(uart_base) };
        uart.init();
        // uart_16550 defaults to a 14-byte receive FIFO trigger. The
        // LoongArch QEMU UART does not reliably raise a receive-timeout IRQ
        // below that threshold, leaving an interactive shell asleep after a
        // few keystrokes. Keep the FIFO enabled but trigger at one byte.
        unsafe {
            ((uart_base + UART_FIFO_CONTROL_OFFSET) as *mut u8)
                .write_volatile(UART_FIFO_ENABLE_CLEAR_TRIGGER_ONE);
        }
        SpinNoIrq::new(uart)
    });
}

struct ConsoleIfImpl;

#[impl_plat_interface]
impl ConsoleIf for ConsoleIfImpl {
    /// Writes bytes to the console from input u8 slice.
    fn write_bytes(bytes: &[u8]) {
        for &c in bytes {
            let mut uart = UART.lock();
            match c {
                b'\n' => {
                    uart.send_raw(b'\r');
                    uart.send_raw(b'\n');
                }
                c => uart.send_raw(c),
            }
        }
    }

    /// Reads bytes from the console into the given mutable slice.
    /// Returns the number of bytes read.
    fn read_bytes(bytes: &mut [u8]) -> usize {
        let mut uart = UART.lock();
        for (i, byte) in bytes.iter_mut().enumerate() {
            match uart.try_receive() {
                Ok(c) => *byte = c,
                Err(_) => return i,
            }
        }
        bytes.len()
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> {
        // QEMU's LoongArch UART receive interrupt is routed through both the
        // PCH-PIC and EIOINTC. That route does not currently wake the generic
        // TTY reader, so poll the UART while userspace waits for console input.
        // This is local to LoongArch QEMU; RISC-V QEMU and VisionFive 2 keep
        // their interrupt-driven console paths.
        None
    }
}
