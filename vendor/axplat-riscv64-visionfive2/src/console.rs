use core::task::Waker;

use axplat::{
    console::ConsoleIf,
    console_rx::ConsoleRxBuffer,
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
const UART_RBR: usize = 0;
const UART_IER: usize = 1;
const UART_IIR_FCR: usize = 2;
const UART_LSR: usize = 5;
const UART_MSR: usize = 6;
const UART_USR: usize = 0x1f;
const UART_IER_RX_AVAILABLE: u32 = 1;
const UART_FCR_ENABLE_AND_CLEAR: u32 = 0x07;
const UART_IIR_NO_INTERRUPT: u32 = 0x01;
const UART_IIR_ID_MASK: u32 = 0x0f;
const UART_IIR_MODEM_STATUS: u32 = 0x00;
const UART_IIR_TX_EMPTY: u32 = 0x02;
const UART_IIR_RX_AVAILABLE: u32 = 0x04;
const UART_IIR_LINE_STATUS: u32 = 0x06;
const UART_IIR_BUSY: u32 = 0x07;
const UART_IIR_RX_TIMEOUT: u32 = 0x0c;
const UART_LSR_DATA_READY: u32 = 1;
const UART_LSR_OVERRUN_ERROR: u32 = 1 << 1;
const RX_BUFFER_SIZE: usize = 8 * 1024;
const RX_DRAIN_LIMIT: usize = 256;
const IRQ_SERVICE_LIMIT: usize = 64;

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

fn register_address(register: usize) -> usize {
    PHYS_VIRT_OFFSET + UART_PADDR + register * UART_REGISTER_STRIDE
}

fn read_register(register: usize) -> u32 {
    unsafe { (register_address(register) as *const u32).read_volatile() }
}

fn write_register(register: usize, value: u32) {
    unsafe { (register_address(register) as *mut u32).write_volatile(value) }
}

fn set_rx_interrupt(enabled: bool) {
    let value = if enabled { UART_IER_RX_AVAILABLE } else { 0 };
    write_register(UART_IER, value);
}

fn set_fifo() {
    write_register(UART_IIR_FCR, UART_FCR_ENABLE_AND_CLEAR);
}

fn line_status() -> u32 {
    read_register(UART_LSR)
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

/// Services every interrupt reason currently asserted by the DesignWare APB
/// UART and returns the tty waiter after newly received bytes were queued.
///
/// In addition to ordinary 16550 RX interrupts, the JH7110 UART can report a
/// busy-detect interrupt which is acknowledged by reading USR. DesignWare can
/// also leave RX-timeout asserted with an empty FIFO; a dummy RBR read is the
/// documented/Linux workaround for that state. Missing either acknowledgement
/// makes the level-triggered PLIC source fire continuously after U-Boot hands
/// the UART to Moss.
fn service_uart_interrupts() -> Option<Waker> {
    let mut pending_waker = None;

    for _ in 0..IRQ_SERVICE_LIMIT {
        let iir = read_register(UART_IIR_FCR);
        let interrupt_id = iir & UART_IIR_ID_MASK;
        // DesignWare's BUSY identification is 0x07 and therefore also has bit
        // zero set. Test the complete ID rather than using the usual 16550
        // `iir & 1` shortcut, or BUSY will be mistaken for "no interrupt".
        if interrupt_id == UART_IIR_NO_INTERRUPT {
            break;
        }

        match interrupt_id {
            UART_IIR_RX_AVAILABLE | UART_IIR_LINE_STATUS | UART_IIR_RX_TIMEOUT => {
                let status = line_status();
                if let Some(waker) = drain_hardware_rx() {
                    pending_waker = Some(waker);
                }
                if interrupt_id == UART_IIR_RX_TIMEOUT && status & UART_LSR_DATA_READY == 0 {
                    // A read is required even though LSR reports no byte. Hold
                    // the shared UART lock so it cannot race a THR write on SMP.
                    let _uart = UART.lock();
                    let _ = read_register(UART_RBR);
                }
            }
            UART_IIR_BUSY => {
                let _ = read_register(UART_USR);
            }
            UART_IIR_MODEM_STATUS => {
                let _ = read_register(UART_MSR);
            }
            UART_IIR_TX_EMPTY => {
                // Moss never enables THRE interrupts. Reassert the intended
                // receive-only IER in case firmware state leaked through.
                set_rx_interrupt(true);
            }
            _ => {
                // Bound recovery for an implementation-specific reason: read
                // all side-effect status registers and drain any available RX.
                let _ = line_status();
                let _ = read_register(UART_MSR);
                let _ = read_register(UART_USR);
                if let Some(waker) = drain_hardware_rx() {
                    pending_waker = Some(waker);
                }
            }
        }
    }

    pending_waker
}

#[cfg(feature = "irq")]
fn handle_uart_interrupt() {
    if let Some(waker) = service_uart_interrupts() {
        waker.wake();
    }
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
pub(crate) fn init_irq() {
    // Keep device interrupts masked while the PLIC handler is installed. Any
    // stale PLIC latch can then invoke a handler which understands all DW APB
    // UART acknowledgement paths instead of trapping in S_EXT forever.
    set_rx_interrupt(false);
    drop(drain_hardware_rx());
    let _ = service_uart_interrupts();
    assert!(
        axplat::irq::register(UART_IRQ, handle_uart_interrupt),
        "failed to register VF2 UART IRQ {UART_IRQ}"
    );
}

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
        Some(UART_IRQ)
    }

    #[cfg(feature = "irq")]
    fn enable_rx_interrupt() {}

    #[cfg(feature = "irq")]
    fn register_rx_waker(waker: &Waker) -> bool {
        let pending = {
            let mut rx = RX.lock();
            rx.register_waker(waker)
        };
        // Enable RDI only after both the platform handler and tty waiter exist.
        // Queued bytes assert the level interrupt immediately after this write.
        set_rx_interrupt(true);
        if pending {
            waker.wake_by_ref();
        }
        true
    }
}
