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
        axlog::with_console_lock(|| axhal::console::write_bytes(buf));
    }

    fn write_echo(&self, buf: &[u8]) {
        // Echo is latency-sensitive and may be inserted between physical UART
        // characters of a normal write. The platform MMIO lock still prevents
        // register races, while ordinary TTY writers remain serialized above.
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
                    // LS2K and VF2 keep their UART interrupt enabled and drain
                    // into a platform ring. Other consoles retain the generic
                    // one-shot IRQ masking behavior as a compatibility path.
                    if !axhal::console::register_rx_waker(&waker) {
                        register_irq_waker(irq, &waker);
                        axhal::console::enable_rx_interrupt();
                    }
                }) as _)
            } else {
                ProcessMode::Polling(Duration::from_millis(10))
            },
        },
    )
}
