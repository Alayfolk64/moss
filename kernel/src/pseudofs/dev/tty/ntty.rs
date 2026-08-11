use alloc::{boxed::Box, sync::Arc};
use core::time::Duration;

use axtask::future::register_irq_waker;
use lazy_static::lazy_static;

use super::{
    Tty,
    terminal::ldisc::{ProcessMode, TtyConfig, TtyRead, TtyWrite},
};

pub type NTtyDriver = Tty<Console, Console>;

#[derive(Clone, Copy)]
pub struct Console;
impl TtyRead for Console {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        axhal::console::read_bytes(buf)
    }
}
impl TtyWrite for Console {
    fn write(&self, buf: &[u8]) {
        axhal::console::write_bytes(buf);
    }
}

lazy_static! {
    /// The default TTY device.
    pub static ref N_TTY: Arc<NTtyDriver> = new_n_tty();
}

fn new_n_tty() -> Arc<NTtyDriver> {
    Tty::new(
        Arc::default(),
        TtyConfig {
            reader: Console,
            writer: Console,
            process_mode: if let Some(irq) = axhal::console::irq_num() {
                ProcessMode::External(Box::new(move |waker| {
                    // Drain once before this callback, then publish the waker
                    // and unmask the interrupt controller before enabling the
                    // NS16550 receive interrupt.  This ordering avoids a lost
                    // first byte and an unhandled level interrupt window.
                    register_irq_waker(irq, &waker);
                    axhal::console::enable_rx_interrupt();
                }) as _)
            } else {
                ProcessMode::Polling(Duration::from_millis(10))
            },
        },
    )
}
