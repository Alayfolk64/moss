use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    future::poll_fn,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
    task::{Poll, Waker},
    time::Duration,
};

use axerrno::{AxError, AxResult};
use axpoll::PollSet;
use axtask::future::block_on_latency_sensitive;
use linux_raw_sys::general::{
    ECHOCTL, ECHOK, ICRNL, IGNCR, ISIG, NOFLSH, VEOF, VERASE, VKILL, VMIN, VTIME,
};
use ringbuf::{
    CachingCons, CachingProd,
    traits::{Consumer, Observer, Producer, Split},
};
use starry_signal::SignalInfo;

use super::{Terminal, termios::Termios2};
use crate::task::send_signal_to_process_group;

const BUF_SIZE: usize = 80;
const ECHO_BUF_SIZE: usize = BUF_SIZE * 3;

type ReadBuf = Arc<ringbuf::StaticRb<u8, BUF_SIZE>>;

/// How should we process inputs?
pub enum ProcessMode {
    /// Process inputs only on call to `read`
    ///
    /// This is the fallback strategy and is rather limited. For instance, you
    /// can't interrupt a running program by Ctrl+C unless it's not blocked on a
    /// `read` call to the terminal, since the signal is emitted only when
    /// inputs are being processed.
    Manual,
    /// Spawns a task that periodically polls the input device.
    ///
    /// This is used when a platform can read its console but cannot provide a
    /// reliable receive interrupt. Unlike [`Self::Manual`], it can still
    /// process terminal signals such as Ctrl+C while another program is in the
    /// foreground.
    Polling(Duration),
    /// Spawns task for processing inputs, relying on external events to wake
    /// up.
    ///
    /// In this mode a dedicated task is spawned to handle inputs. When there's
    /// nothing to read the argument is invoked to register rx waker.
    External(Box<dyn Fn(Waker) + Send + Sync>),
    /// Do not process inputs.
    ///
    /// This is only used by the master side of pseudo tty. The argument is the
    /// [`PollSet`] for incoming data.
    None(Arc<PollSet>),
}

pub struct TtyConfig<R, W> {
    pub reader: R,
    pub writer: W,
    pub process_mode: ProcessMode,
}

pub trait TtyRead: Send + Sync + 'static {
    fn read(&mut self, buf: &mut [u8]) -> usize;
}
pub trait TtyWrite: Send + Sync + 'static {
    fn write(&self, buf: &[u8]);

    /// Writes terminal-generated echo output.
    ///
    /// Devices may override this to let interactive echo pass a long normal
    /// write at a safe hardware boundary. PTYs retain ordinary write ordering.
    fn write_echo(&self, buf: &[u8]) {
        self.write(buf);
    }
}

struct EchoBuffer {
    bytes: [u8; ECHO_BUF_SIZE],
    len: usize,
}

impl EchoBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; ECHO_BUF_SIZE],
            len: 0,
        }
    }

    fn extend(&mut self, bytes: &[u8]) {
        // InputReader consumes at most BUF_SIZE input bytes per poll and each
        // byte expands to at most the three-byte erase sequence.
        assert!(self.len + bytes.len() <= self.bytes.len());
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    fn flush<W: TtyWrite>(&mut self, writer: &W) {
        if self.len != 0 {
            writer.write_echo(&self.bytes[..self.len]);
            self.len = 0;
        }
    }
}

fn buffer_echo_char(term: &Termios2, ch: u8, output: &mut EchoBuffer) {
    match ch {
        b'\n' => output.extend(b"\n"),
        b'\r' => output.extend(b"\r\n"),
        ch if ch == term.special_char(VERASE) => output.extend(b"\x08 \x08"),
        ch if ch == b' ' || ch.is_ascii_graphic() => output.extend(&[ch]),
        ch if ch.is_ascii_control() && term.has_lflag(ECHOCTL) => {
            output.extend(&[b'^', (ch + 0x40)]);
        }
        other => {
            warn!("Ignored echo char: {other:#x}");
        }
    }
}

struct InputReader<R, W> {
    terminal: Arc<Terminal>,

    reader: R,
    writer: W,

    buf_tx: CachingProd<ReadBuf>,
    read_buf: [u8; BUF_SIZE],
    read_range: Range<usize>,

    line_buf: Vec<u8>,
    line_read: Option<usize>,
    clear_line_buf: Arc<AtomicBool>,
    flush_input: Arc<AtomicBool>,
}
impl<R: TtyRead, W: TtyWrite> InputReader<R, W> {
    pub fn poll(&mut self) -> bool {
        let mut echo = EchoBuffer::new();
        if self.clear_line_buf.swap(false, Ordering::Relaxed) {
            self.line_buf.clear();
        }
        if self.read_range.is_empty() {
            let read = self.reader.read(&mut self.read_buf);
            self.read_range = 0..read;
        }
        let term = self.terminal.load_termios();
        let mut sent = 0;
        loop {
            if let Some(offset) = &mut self.line_read {
                let read = self.buf_tx.push_slice(&self.line_buf[*offset..]);
                if read == 0 {
                    break;
                }
                sent += read;
                *offset += read;
                if *offset == self.line_buf.len() {
                    self.line_read = None;
                    self.line_buf.clear();
                }
                continue;
            }
            if self.buf_tx.is_full() || self.read_range.is_empty() {
                break;
            }
            let mut ch = self.read_buf[self.read_range.start];
            self.read_range.start += 1;

            if ch == b'\r' {
                if term.has_iflag(IGNCR) {
                    continue;
                }
                if term.has_iflag(ICRNL) {
                    ch = b'\n';
                }
            }

            if self.check_send_signal(&term, ch) {
                if term.echo() {
                    buffer_echo_char(&term, ch, &mut echo);
                }
                // ISIG characters are terminal controls, not input bytes.
                continue;
            }
            if term.echo() {
                buffer_echo_char(&term, ch, &mut echo);
            }
            if !term.canonical() {
                self.buf_tx.try_push(ch).unwrap();
                sent += 1;
                continue;
            }

            // Canonical mode
            if term.has_lflag(ECHOK) && ch == term.special_char(VKILL) {
                self.line_buf.clear();
                continue;
            }
            if ch == term.special_char(VERASE) {
                self.line_buf.pop();
                continue;
            }

            if term.is_eol(ch) || ch == term.special_char(VEOF) {
                if ch != term.special_char(VEOF) {
                    self.line_buf.push(ch);
                }
                if !self.line_buf.is_empty() {
                    self.line_read = Some(0);
                }
                continue;
            }

            if ch == b' ' || ch.is_ascii_graphic() {
                self.line_buf.push(ch);
                continue;
            }
        }

        echo.flush(&self.writer);
        sent > 0
    }

    fn check_send_signal(&mut self, term: &Termios2, ch: u8) -> bool {
        // POSIX ISIG is independent of ICANON: Ctrl+C must interrupt a
        // foreground raw-mode program as well as a canonical-mode shell.
        if !term.has_lflag(ISIG) {
            return false;
        }
        let Some(signo) = term.signo_for(ch) else {
            return false;
        };

        if !term.has_lflag(NOFLSH) {
            // Flush canonical state immediately and ask the consumer side to
            // discard bytes that were committed before the signal character.
            self.line_buf.clear();
            self.line_read = None;
            self.read_range = 0..0;
            self.flush_input.store(true, Ordering::Release);
        }

        if let Some(pg) = self.terminal.job_control.foreground() {
            let sig = SignalInfo::new_kernel(signo);
            if let Err(err) = send_signal_to_process_group(pg.pgid(), Some(sig))
                && err != AxError::NoSuchProcess
            {
                warn!("Failed to send terminal signal: {err:?}");
            }
        }
        true
    }
}

struct SimpleReader<R> {
    reader: R,
    read_buf: [u8; BUF_SIZE],
    buf_tx: CachingProd<ReadBuf>,
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use kspin::SpinNoIrq;
    use linux_raw_sys::general::{VERASE, VINTR};

    use super::{EchoBuffer, TtyWrite, buffer_echo_char};
    use crate::pseudofs::dev::tty::terminal::termios::Termios2;

    struct RecordingWriter(SpinNoIrq<Vec<Vec<u8>>>);

    impl Default for RecordingWriter {
        fn default() -> Self {
            Self(SpinNoIrq::new(Vec::new()))
        }
    }

    impl TtyWrite for RecordingWriter {
        fn write(&self, buf: &[u8]) {
            self.0.lock().push(buf.to_vec());
        }
    }

    #[test]
    fn batches_printable_and_control_echo_into_one_write() {
        let term = Termios2::default();
        let writer = RecordingWriter::default();
        let mut output = EchoBuffer::new();

        buffer_echo_char(&term, b'a', &mut output);
        buffer_echo_char(&term, b'\n', &mut output);
        buffer_echo_char(&term, term.special_char(VERASE), &mut output);
        buffer_echo_char(&term, term.special_char(VINTR), &mut output);
        output.flush(&writer);

        assert_eq!(*writer.0.lock(), vec![b"a\n\x08 \x08^C".to_vec()]);
    }

    #[test]
    fn holds_the_worst_case_input_batch_without_partial_writes() {
        let term = Termios2::default();
        let writer = RecordingWriter::default();
        let mut output = EchoBuffer::new();

        for _ in 0..super::BUF_SIZE {
            buffer_echo_char(&term, term.special_char(VERASE), &mut output);
        }
        output.flush(&writer);

        let writes = writer.0.lock();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].len(), super::ECHO_BUF_SIZE);
    }
}
impl<R: TtyRead> SimpleReader<R> {
    pub fn poll(&mut self) {
        let read = self.reader.read(&mut self.read_buf);
        for ch in &self.read_buf[..read] {
            if *ch == b'\n' {
                let _ = self.buf_tx.try_push(b'\r');
            }
            let _ = self.buf_tx.try_push(*ch);
        }
    }
}

enum Processor<R, W> {
    Manual(InputReader<R, W>),
    External(Arc<PollSet>),
    None(SimpleReader<R>, Arc<PollSet>),
}

pub struct LineDiscipline<R, W> {
    terminal: Arc<Terminal>,
    buf_rx: CachingCons<ReadBuf>,
    poll_tx: Arc<PollSet>,
    clear_line_buf: Arc<AtomicBool>,
    flush_input: Arc<AtomicBool>,
    processor: Processor<R, W>,
}

impl<R: TtyRead, W: TtyWrite> LineDiscipline<R, W> {
    pub fn new(terminal: Arc<Terminal>, config: TtyConfig<R, W>) -> Self {
        let (buf_tx, buf_rx) = ReadBuf::default().split();

        let clear_line_buf = Arc::new(AtomicBool::new(false));
        let flush_input = Arc::new(AtomicBool::new(false));
        let mut reader = InputReader {
            terminal: terminal.clone(),

            reader: config.reader,
            writer: config.writer,

            buf_tx,
            read_buf: [0; BUF_SIZE],
            read_range: 0..0,

            line_buf: Vec::new(),
            line_read: None,
            clear_line_buf: clear_line_buf.clone(),
            flush_input: flush_input.clone(),
        };

        let poll_tx = Arc::new(PollSet::new());
        let processor = match config.process_mode {
            ProcessMode::Manual => Processor::Manual(reader),
            ProcessMode::Polling(interval) => {
                let poll_rx = Arc::new(PollSet::new());
                axtask::spawn_with_name(
                    {
                        let poll_rx = poll_rx.clone();
                        move || loop {
                            while reader.poll() {
                                poll_rx.wake();
                            }
                            axtask::sleep(interval);
                        }
                    },
                    "tty-poller".into(),
                );
                Processor::External(poll_rx)
            }
            ProcessMode::External(register) => {
                let poll_rx = Arc::new(PollSet::new());
                axtask::spawn_with_name(
                    {
                        let poll_rx = poll_rx.clone();
                        let poll_tx = poll_tx.clone();
                        move || {
                            block_on_latency_sensitive(poll_fn(|cx| {
                                while reader.poll() {
                                    poll_rx.wake();
                                }
                                poll_tx.register(cx.waker());
                                register(cx.waker().clone());
                                while reader.poll() {
                                    poll_rx.wake();
                                }
                                Poll::Pending
                            }))
                        }
                    },
                    "tty-reader".into(),
                );
                Processor::External(poll_rx)
            }
            ProcessMode::None(poll_rx) => {
                // Destruct the reader here
                Processor::None(
                    SimpleReader {
                        reader: reader.reader,
                        read_buf: [0; BUF_SIZE],
                        buf_tx: reader.buf_tx,
                    },
                    poll_rx,
                )
            }
        };
        Self {
            terminal,
            buf_rx,
            poll_tx,
            clear_line_buf,
            flush_input,
            processor,
        }
    }

    fn apply_signal_flush(&mut self) {
        if self.flush_input.swap(false, Ordering::Acquire) {
            self.buf_rx.clear();
            self.poll_tx.wake();
        }
    }

    pub fn drain_input(&mut self) {
        self.buf_rx.clear();
        self.clear_line_buf.store(true, Ordering::Relaxed);
    }

    pub fn poll_read(&mut self) -> bool {
        self.apply_signal_flush();
        match &mut self.processor {
            Processor::Manual(reader) => {
                reader.poll();
            }
            Processor::None(reader, _) => reader.poll(),
            _ => {}
        }
        !self.buf_rx.is_empty()
    }

    pub fn register_rx_waker(&self, waker: &Waker) {
        match &self.processor {
            Processor::Manual(_) => {
                waker.wake_by_ref();
            }
            Processor::External(set) | Processor::None(_, set) => {
                set.register(waker);
            }
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> AxResult<usize> {
        self.apply_signal_flush();
        if buf.is_empty() {
            return Ok(0);
        }
        if matches!(self.processor, Processor::None(_, _)) {
            let read = self.buf_rx.pop_slice(buf);
            return if read == 0 {
                Err(AxError::WouldBlock)
            } else {
                Ok(read)
            };
        }

        let term = self.terminal.termios.lock().clone();
        let vmin = if term.canonical() {
            1
        } else {
            let vtime = term.special_char(VTIME);
            if vtime > 0 {
                todo!();
            }
            term.special_char(VMIN) as usize
        };

        let required = vmin.min(buf.len());
        if let Processor::Manual(reader) = &mut self.processor {
            reader.poll();
        }
        if self.buf_rx.occupied_len() < required {
            return Err(AxError::WouldBlock);
        }

        let read = self.buf_rx.pop_slice(buf);
        if read != 0 {
            self.poll_tx.wake();
        }
        Ok(read)
    }
}
