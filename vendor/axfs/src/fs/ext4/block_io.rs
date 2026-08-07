use alloc::{boxed::Box, collections::BTreeMap, sync::Arc};
use core::{
    future::poll_fn,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    task::Poll,
};

use axdriver::{
    AxBlockDevice,
    prelude::{BlockDriverOps, BlockRequest, BlockRequestId, DevError, DevResult},
};
use axsync::Mutex;
use axtask::WaitQueue;

static ASYNC_BLOCK_IO_READY: AtomicBool = AtomicBool::new(false);

pub(super) fn enable_async_block_io() {
    ASYNC_BLOCK_IO_READY.store(true, Ordering::Release);
}

enum BlockDeviceHolder {
    Runtime(AxBlockDevice),
    Driver(Box<dyn BlockDriverOps>),
}

impl BlockDeviceHolder {
    fn device(&self) -> &dyn BlockDriverOps {
        match self {
            Self::Runtime(device) => device,
            Self::Driver(device) => device.as_ref(),
        }
    }
}

struct Completion {
    result: Mutex<Option<DevResult>>,
    wait: WaitQueue,
}

impl Completion {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            wait: WaitQueue::new(),
        }
    }

    fn finish(&self, result: DevResult) {
        *self.result.lock() = Some(result);
        self.wait.notify_one(false);
    }

    fn wait(&self) -> DevResult {
        self.wait.wait_until(|| self.result.lock().is_some());
        self.result
            .lock()
            .take()
            .expect("completed block request lost its result")
    }
}

struct DeviceState {
    device: BlockDeviceHolder,
    pending: BTreeMap<BlockRequestId, Arc<Completion>>,
    flushing: bool,
}

struct SharedBlockDeviceInner {
    state: Mutex<DeviceState>,
    next_request: AtomicU64,
    outstanding: AtomicUsize,
    queue_depth: usize,
    asynchronous: bool,
    irq: Option<usize>,
    completion_started: AtomicBool,
    work: WaitQueue,
    queue_space: WaitQueue,
}

/// Serializes queue access without holding the lock for the I/O duration.
#[derive(Clone)]
pub(crate) struct SharedBlockDevice(Arc<SharedBlockDeviceInner>);

impl SharedBlockDevice {
    pub(crate) fn runtime(device: AxBlockDevice) -> Self {
        Self::new(BlockDeviceHolder::Runtime(device))
    }

    pub(crate) fn driver(device: impl BlockDriverOps + 'static) -> Self {
        Self::new(BlockDeviceHolder::Driver(Box::new(device)))
    }

    fn new(device: BlockDeviceHolder) -> Self {
        let queue_depth = device.device().max_in_flight().max(1);
        let asynchronous = device.device().supports_async();
        let irq = device.device().irq_num();
        Self(Arc::new(SharedBlockDeviceInner {
            state: Mutex::new(DeviceState {
                device,
                pending: BTreeMap::new(),
                flushing: false,
            }),
            next_request: AtomicU64::new(1),
            outstanding: AtomicUsize::new(0),
            queue_depth,
            asynchronous,
            irq,
            completion_started: AtomicBool::new(false),
            work: WaitQueue::new(),
            queue_space: WaitQueue::new(),
        }))
    }

    fn ensure_completion_task(&self) {
        if !self.0.asynchronous || self.0.completion_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = self.clone();
        if self.0.irq.is_some() {
            self.0.state.lock().device.device().enable_interrupts();
        }
        axtask::spawn(move || this.completion_loop());
    }

    fn complete_one(&self) -> bool {
        let completed = {
            let mut state = self.0.state.lock();
            state.device.device().ack_interrupt();
            state
                .device
                .device()
                .poll_complete()
                .map(|(id, result)| (state.pending.remove(&id), result))
        };

        let Some((completion, result)) = completed else {
            return false;
        };
        let remaining = self.0.outstanding.fetch_sub(1, Ordering::AcqRel) - 1;
        if remaining == 0 {
            self.0.queue_space.notify_all(false);
        } else {
            self.0.queue_space.notify_one(false);
        }
        if let Some(completion) = completion {
            completion.finish(result);
        } else {
            error!("block driver completed an unknown request");
        }
        true
    }

    fn wait_for_interrupt(&self, irq: usize) {
        axtask::future::block_on(poll_fn(|context| {
            axtask::future::register_irq_waker(irq, context.waker());
            if self.0.outstanding.load(Ordering::Acquire) == 0 || self.complete_one() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }));
    }

    fn completion_loop(&self) -> ! {
        loop {
            self.0
                .work
                .wait_until(|| self.0.outstanding.load(Ordering::Acquire) != 0);

            while self.0.outstanding.load(Ordering::Acquire) != 0 {
                if self.complete_one() {
                    continue;
                }
                if let Some(irq) = self.0.irq {
                    self.wait_for_interrupt(irq);
                } else {
                    axtask::yield_now();
                }
            }
        }
    }

    fn queue_has_space(&self) -> bool {
        let state = self.0.state.lock();
        !state.flushing && state.pending.len() < self.0.queue_depth
    }

    unsafe fn submit_sync(
        &self,
        write: bool,
        block_id: u64,
        mut buffer: NonNull<[u8]>,
    ) -> DevResult {
        let state = self.0.state.lock();
        if write {
            state
                .device
                .device()
                .write_block(block_id, unsafe { buffer.as_ref() })
        } else {
            state
                .device
                .device()
                .read_block(block_id, unsafe { buffer.as_mut() })
        }
    }

    unsafe fn submit(&self, write: bool, block_id: u64, buffer: NonNull<[u8]>) -> DevResult {
        // Rootfs is mounted before secondary run queues and interrupts are
        // ready, so bootstrap traffic remains synchronous.
        if !ASYNC_BLOCK_IO_READY.load(Ordering::Acquire) {
            return unsafe { self.submit_sync(write, block_id, buffer) };
        }
        self.ensure_completion_task();

        loop {
            self.0.queue_space.wait_until(|| self.queue_has_space());

            let id = self.0.next_request.fetch_add(1, Ordering::Relaxed);
            let completion = Arc::new(Completion::new());
            let submission = {
                let mut state = self.0.state.lock();
                if state.flushing || state.pending.len() >= self.0.queue_depth {
                    None
                } else {
                    state.pending.insert(id, completion.clone());
                    let submitted = if write {
                        unsafe { state.device.device().submit_write(id, block_id, buffer) }
                    } else {
                        unsafe { state.device.device().submit_read(id, block_id, buffer) }
                    };
                    Some(match submitted {
                        Ok(BlockRequest::Complete) => {
                            state.pending.remove(&id);
                            Ok(false)
                        }
                        Ok(BlockRequest::Pending(returned)) if returned == id => {
                            self.0.outstanding.fetch_add(1, Ordering::Release);
                            Ok(true)
                        }
                        Ok(BlockRequest::Pending(_)) => {
                            state.pending.remove(&id);
                            Err(DevError::BadState)
                        }
                        Err(err) => {
                            state.pending.remove(&id);
                            Err(err)
                        }
                    })
                }
            };

            let Some(submission) = submission else {
                axtask::yield_now();
                continue;
            };
            if submission? {
                self.0.work.notify_one(false);
                // Fast devices may publish a used descriptor before the
                // completion task runs; reap it immediately when possible.
                self.complete_one();
                return completion.wait();
            }
            self.0.queue_space.notify_one(false);
            return Ok(());
        }
    }

    pub(crate) fn read_blocks(&self, block_id: u64, buf: &mut [u8]) -> DevResult {
        if buf.is_empty() {
            return Ok(());
        }
        unsafe { self.submit(false, block_id, NonNull::from(buf)) }
    }

    pub(crate) fn write_blocks(&self, block_id: u64, buf: &[u8]) -> DevResult {
        if buf.is_empty() {
            return Ok(());
        }
        unsafe { self.submit(true, block_id, NonNull::from(buf)) }
    }

    pub(crate) fn num_blocks(&self) -> u64 {
        self.0.state.lock().device.device().num_blocks()
    }

    pub(crate) fn block_size(&self) -> usize {
        self.0.state.lock().device.device().block_size()
    }

    pub(crate) fn flush(&self) -> DevResult {
        loop {
            self.0
                .queue_space
                .wait_until(|| self.0.outstanding.load(Ordering::Acquire) == 0);

            let mut state = self.0.state.lock();
            if state.flushing || !state.pending.is_empty() {
                drop(state);
                axtask::yield_now();
                continue;
            }
            state.flushing = true;
            let result = state.device.device().flush();
            state.flushing = false;
            drop(state);
            self.0.queue_space.notify_all(false);
            return result;
        }
    }
}
