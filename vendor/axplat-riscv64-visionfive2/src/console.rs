use axplat::{
    console::ConsoleIf,
    mem::{pa, phys_to_virt},
};
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use uart_16550::MmioSerialPort;

use crate::config::devices::UART_PADDR;

static UART: LazyInit<SpinNoIrq<MmioSerialPort>> = LazyInit::new();

pub(crate) fn init_early() {
    UART.init_once({
        // JH7110 exposes the 8250 registers at a four-byte stride. U-Boot has
        // already configured the clock and baud rate, so do not reinitialize it.
        let uart = unsafe {
            MmioSerialPort::new_with_stride(phys_to_virt(pa!(UART_PADDR)).as_usize(), 4)
        };
        SpinNoIrq::new(uart)
    });
}

struct ConsoleIfImpl;

#[impl_plat_interface]
impl ConsoleIf for ConsoleIfImpl {
    fn write_bytes(bytes: &[u8]) {
        for &byte in bytes {
            let mut uart = UART.lock();
            if byte == b'\n' {
                uart.send_raw(b'\r');
            }
            uart.send_raw(byte);
        }
    }

    fn read_bytes(bytes: &mut [u8]) -> usize {
        let mut uart = UART.lock();
        for (index, byte) in bytes.iter_mut().enumerate() {
            match uart.try_receive() {
                Ok(received) => *byte = received,
                Err(_) => return index,
            }
        }
        bytes.len()
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> {
        // The first-stage port deliberately uses polling UART. JH7110 external
        // interrupt routing can be enabled independently later.
        None
    }
}
