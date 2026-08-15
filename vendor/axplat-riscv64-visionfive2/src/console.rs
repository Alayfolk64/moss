use core::task::Waker;

use axplat::{
    console::ConsoleIf,
    console_rx::ConsoleRxBuffer,
    mem::{pa, phys_to_virt},
};
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use uart_16550::MmioSerialPort;

use crate::config::{devices::UART_PADDR, plat::PHYS_VIRT_OFFSET};

const UART_REGISTER_STRIDE: usize = 4;
const UART_IER_OFFSET: usize = UART_REGISTER_STRIDE;
const UART_FCR_OFFSET: usize = 2 * UART_REGISTER_STRIDE;
const UART_LSR_OFFSET: usize = 5 * UART_REGISTER_STRIDE;
const UART_IER_RX_AVAILABLE: u32 = 1;
const UART_FCR_ENABLE_AND_CLEAR: u32 = 0x07;
const UART_LSR_OVERRUN_ERROR: u32 = 1 << 1;
const RX_BUFFER_SIZE: usize = 8 * 1024;
const RX_DRAIN_LIMIT: usize = 256;

struct ConsoleUart {
    port: MmioSerialPort,
}

impl ConsoleUart {
    fn new(base: usize) -> Self {
        Self {
            port: unsafe { MmioSerialPort::new_with_stride(base, UART_REGISTER_STRIDE) },
        }
    }
}

static UART: LazyInit<SpinNoIrq<ConsoleUart>> = LazyInit::new();
static RX: SpinNoIrq<ConsoleRxBuffer<RX_BUFFER_SIZE>> = SpinNoIrq::new(ConsoleRxBuffer::new());

fn set_rx_interrupt(enabled: bool) {
    let value = if enabled { UART_IER_RX_AVAILABLE } else { 0 };
    unsafe {
        ((PHYS_VIRT_OFFSET + UART_PADDR + UART_IER_OFFSET) as *mut u32).write_volatile(value);
    }
}

fn set_fifo() {
    unsafe {
        ((PHYS_VIRT_OFFSET + UART_PADDR + UART_FCR_OFFSET) as *mut u32)
            .write_volatile(UART_FCR_ENABLE_AND_CLEAR);
    }
}

fn line_status() -> u32 {
    unsafe { ((PHYS_VIRT_OFFSET + UART_PADDR + UART_LSR_OFFSET) as *const u32).read_volatile() }
}

fn drain_hardware_rx() -> Option<Waker> {
    let mut batch = [0; RX_DRAIN_LIMIT];
    let (received, hardware_overruns) = {
        let mut uart = UART.lock();
        let mut received = 0;
        let mut hardware_overruns = 0usize;
        while received < batch.len() {
            if line_status() & UART_LSR_OVERRUN_ERROR != 0 {
                hardware_overruns = hardware_overruns.saturating_add(1);
            }
            let Ok(byte) = uart.port.try_receive() else {
                break;
            };
            batch[received] = byte;
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
    set_fifo();
}

#[cfg(feature = "irq")]
pub(crate) fn init_irq() {}

struct ConsoleIfImpl;

#[impl_plat_interface]
impl ConsoleIf for ConsoleIfImpl {
    fn write_bytes(bytes: &[u8]) {
        // Normal TTY writes and log records are serialized by their callers.
        // Keep the IRQ-off MMIO section to one physical character so an RX IRQ
        // or high-priority echo can run between characters of a long write.
        for &byte in bytes {
            if byte == b'\n' {
                UART.lock().port.send_raw(b'\r');
            }
            UART.lock().port.send_raw(byte);
        }
    }

    fn read_bytes(bytes: &mut [u8]) -> usize {
        drop(drain_hardware_rx());
        RX.lock().pop_slice(bytes)
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> {
        // U-Boot can leave the JH7110 UART source pending even after its FIFO
        // has been drained. Enabling that level-triggered PLIC source traps the
        // CPU in S_EXT. Keep the device IER masked and use N_TTY's polling path;
        // SD/MMC, GMAC, timer, and IPI interrupts remain enabled normally.
        None
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
