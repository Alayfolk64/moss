use alloc::{
    borrow::Cow,
    collections::BTreeMap,
    format,
    sync::{Arc, Weak},
};
use core::{
    mem,
    sync::atomic::{AtomicBool, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult};
use axpoll::{IoEvents, PollSet, Pollable};
use axsync::Mutex;
use axtask::{
    current,
    future::{block_on, poll_io},
};
use linux_raw_sys::{
    general::{O_NONBLOCK, O_RDONLY, O_WRONLY, S_IFIFO},
    ioctl::FIONREAD,
};
use memory_addr::PAGE_SIZE_4K;
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Producer},
};
use spin::RwLock;
use starry_signal::{SignalInfo, Signo};
use starry_vm::VmMutPtr;

use super::{FileLike, Kstat, notify_async_io};
use crate::{
    file::{IoDst, IoSrc},
    task::{AsThread, send_signal_to_process},
};

const RING_BUFFER_INIT_SIZE: usize = 65536; // 64 KiB
pub const PIPE_MAX_SIZE: usize = 1048576; // 1 MiB
type FifoKey = (u64, u64);

static FIFO_PIPES: RwLock<BTreeMap<FifoKey, Weak<Shared>>> = RwLock::new(BTreeMap::new());

struct Shared {
    buffer: Mutex<HeapRb<u8>>,
    poll_rx: PollSet,
    poll_tx: PollSet,
    poll_close: PollSet,
}

pub struct Pipe {
    read_side: bool,
    shared: Arc<Shared>,
    non_blocking: AtomicBool,
    fifo_key: Option<FifoKey>,
}
impl Drop for Pipe {
    fn drop(&mut self) {
        self.shared.poll_close.wake();
    }
}

impl Pipe {
    pub fn new() -> (Pipe, Pipe) {
        let shared = Arc::new(Shared {
            buffer: Mutex::new(HeapRb::new(RING_BUFFER_INIT_SIZE)),
            poll_rx: PollSet::new(),
            poll_tx: PollSet::new(),
            poll_close: PollSet::new(),
        });
        let read_end = Pipe {
            read_side: true,
            shared: shared.clone(),
            non_blocking: AtomicBool::new(false),
            fifo_key: None,
        };
        let write_end = Pipe {
            read_side: false,
            shared,
            non_blocking: AtomicBool::new(false),
            fifo_key: None,
        };
        (read_end, write_end)
    }

    pub fn new_fifo(device: u64, inode: u64, read_side: bool) -> Self {
        let key = (device, inode);
        let shared = {
            let mut pipes = FIFO_PIPES.write();
            if let Some(shared) = pipes.get(&key).and_then(Weak::upgrade) {
                shared
            } else {
                let shared = Arc::new(Shared {
                    buffer: Mutex::new(HeapRb::new(RING_BUFFER_INIT_SIZE)),
                    poll_rx: PollSet::new(),
                    poll_tx: PollSet::new(),
                    poll_close: PollSet::new(),
                });
                pipes.insert(key, Arc::downgrade(&shared));
                shared
            }
        };

        Self {
            read_side,
            shared,
            non_blocking: AtomicBool::new(false),
            fifo_key: Some(key),
        }
    }

    pub const fn fifo_key(&self) -> Option<FifoKey> {
        self.fifo_key
    }

    pub const fn is_read(&self) -> bool {
        self.read_side
    }

    pub const fn is_write(&self) -> bool {
        !self.read_side
    }

    pub fn closed(&self) -> bool {
        Arc::strong_count(&self.shared) == 1
    }

    pub fn capacity(&self) -> usize {
        self.shared.buffer.lock().capacity().get()
    }

    pub fn resize(&self, new_size: usize) -> AxResult<usize> {
        let new_size = new_size.div_ceil(PAGE_SIZE_4K).max(1) * PAGE_SIZE_4K;

        let mut buffer = self.shared.buffer.lock();
        if new_size == buffer.capacity().get() {
            return Ok(new_size);
        }
        if new_size < buffer.occupied_len() {
            return Err(AxError::ResourceBusy);
        }
        let old_buffer = mem::replace(&mut *buffer, HeapRb::new(new_size));
        let (left, right) = old_buffer.as_slices();
        buffer.push_slice(left);
        buffer.push_slice(right);
        Ok(new_size)
    }

    pub(super) fn async_key(&self) -> usize {
        Arc::as_ptr(&self.shared) as usize
    }

    pub fn write_once(&self, src: &mut IoSrc) -> AxResult<usize> {
        self.try_write(src)
    }

    fn try_read(&self, dst: &mut IoDst) -> AxResult<usize> {
        let read = {
            let cons = self.shared.buffer.lock();
            let (left, right) = cons.as_slices();
            let mut count = dst.write(left)?;
            if count >= left.len() {
                count += dst.write(right)?;
            }
            unsafe { cons.advance_read_index(count) };
            count
        };
        if read > 0 {
            self.shared.poll_tx.wake();
            Ok(read)
        } else if self.closed() {
            Ok(0)
        } else {
            Err(AxError::WouldBlock)
        }
    }

    fn try_write(&self, src: &mut IoSrc) -> AxResult<usize> {
        if self.closed() {
            raise_pipe();
            return Err(AxError::BrokenPipe);
        }

        let written = {
            let mut prod = self.shared.buffer.lock();
            let (left, right) = prod.vacant_slices_mut();
            let left = unsafe {
                core::slice::from_raw_parts_mut(left.as_mut_ptr().cast::<u8>(), left.len())
            };
            let right = unsafe {
                core::slice::from_raw_parts_mut(right.as_mut_ptr().cast::<u8>(), right.len())
            };
            let mut count = src.read(left)?;
            if count >= left.len() {
                count += src.read(right)?;
            }
            unsafe { prod.advance_write_index(count) };
            count
        };
        if written == 0 {
            return Err(AxError::WouldBlock);
        }

        self.shared.poll_rx.wake();
        notify_async_io(self.async_key());
        Ok(written)
    }
}

fn raise_pipe() {
    let curr = current();
    send_signal_to_process(
        curr.as_thread().proc_data.proc.pid(),
        Some(SignalInfo::new_kernel(Signo::SIGPIPE)),
    )
    .expect("Failed to send SIGPIPE");
}

impl FileLike for Pipe {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        if !self.is_read() {
            return Err(AxError::BadFileDescriptor);
        }
        if dst.is_full() {
            return Ok(0);
        }

        if current().take_interrupt() {
            return Err(AxError::Interrupted);
        }

        let nonblocking = self.nonblocking();
        match self.try_read(dst) {
            Err(AxError::WouldBlock) if nonblocking => Err(AxError::WouldBlock),
            Err(AxError::WouldBlock) => {
                block_on(poll_io(self, IoEvents::IN, false, || self.try_read(dst)))
            }
            result => result,
        }
    }

    fn write(&self, src: &mut IoSrc) -> AxResult<usize> {
        if !self.is_write() {
            return Err(AxError::BadFileDescriptor);
        }
        let size = src.remaining();
        if size == 0 {
            return Ok(0);
        }

        let mut total_written = 0;

        if current().take_interrupt() {
            return Err(AxError::Interrupted);
        }

        let nonblocking = self.nonblocking();
        match self.try_write(src) {
            Ok(written) => {
                total_written = written;
                if total_written == size || nonblocking {
                    return Ok(total_written);
                }
            }
            Err(AxError::WouldBlock) if nonblocking => return Err(AxError::WouldBlock),
            Err(AxError::WouldBlock) => {}
            Err(err) => return Err(err),
        }

        block_on(poll_io(self, IoEvents::OUT, false, || {
            match self.try_write(src) {
                Ok(written) => total_written += written,
                Err(AxError::WouldBlock) => return Err(AxError::WouldBlock),
                Err(err) => return Err(err),
            }
            if total_written == size {
                Ok(total_written)
            } else {
                Err(AxError::WouldBlock)
            }
        }))
    }

    fn stat(&self) -> AxResult<Kstat> {
        Ok(Kstat {
            mode: S_IFIFO | if self.is_read() { 0o444 } else { 0o222 },
            ..Default::default()
        })
    }

    fn path(&self) -> Cow<'_, str> {
        format!("pipe:[{}]", self as *const _ as usize).into()
    }

    fn set_nonblocking(&self, nonblocking: bool) -> AxResult {
        self.non_blocking.store(nonblocking, Ordering::Release);
        Ok(())
    }

    fn nonblocking(&self) -> bool {
        self.non_blocking.load(Ordering::Acquire)
    }

    fn status_flags(&self) -> u32 {
        let mut flags = if self.is_read() { O_RDONLY } else { O_WRONLY };
        if self.nonblocking() {
            flags |= O_NONBLOCK;
        }
        flags
    }

    fn set_status_flags(&self, flags: u32) -> AxResult {
        self.set_nonblocking(flags & O_NONBLOCK != 0)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> AxResult<usize> {
        match cmd {
            FIONREAD => {
                (arg as *mut u32).vm_write(self.shared.buffer.lock().occupied_len() as u32)?;
                Ok(0)
            }
            _ => Err(AxError::NotATty),
        }
    }
}

impl Pollable for Pipe {
    fn poll(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        let buf = self.shared.buffer.lock();
        if self.read_side {
            events.set(IoEvents::IN, buf.occupied_len() > 0);
            events.set(IoEvents::HUP, self.closed());
        } else {
            events.set(IoEvents::OUT, buf.vacant_len() >= PAGE_SIZE_4K);
        }
        events
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.contains(IoEvents::IN) {
            self.shared.poll_rx.register(context.waker());
        }
        if events.contains(IoEvents::OUT) {
            self.shared.poll_tx.register(context.waker());
        }
        self.shared.poll_close.register(context.waker());
    }
}
