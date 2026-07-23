use core::ptr::NonNull;

use axdriver_base::{BaseDriverOps, DevResult, DeviceType};
use axdriver_block::{BlockDriverOps, BlockRequest, BlockRequestId};
use virtio_drivers::{
    Hal,
    device::blk::{BlkReq, BlkResp, VirtIOBlk as InnerDev},
    transport::Transport,
};

use crate::as_dev_err;

const MAX_PENDING_REQUESTS: usize = 16;

#[derive(Clone, Copy)]
enum PendingOp {
    Read,
    Write,
}

struct PendingRequest {
    id: BlockRequestId,
    token: u16,
    op: PendingOp,
    buffer: NonNull<[u8]>,
    request: BlkReq,
    response: BlkResp,
}

impl PendingRequest {
    fn new(id: BlockRequestId, op: PendingOp, buffer: NonNull<[u8]>) -> Self {
        Self {
            id,
            token: 0,
            op,
            buffer,
            request: BlkReq::default(),
            response: BlkResp::default(),
        }
    }
}

/// The VirtIO block device driver.
pub struct VirtIoBlkDev<H: Hal, T: Transport> {
    inner: InnerDev<H, T>,
    pending: [Option<PendingRequest>; MAX_PENDING_REQUESTS],
    irq: Option<usize>,
}

unsafe impl<H: Hal, T: Transport> Send for VirtIoBlkDev<H, T> {}
unsafe impl<H: Hal, T: Transport> Sync for VirtIoBlkDev<H, T> {}

impl<H: Hal, T: Transport> VirtIoBlkDev<H, T> {
    /// Creates a new driver instance and initializes the device, or returns
    /// an error if any step fails.
    pub fn try_new(transport: T) -> DevResult<Self> {
        Self::try_new_with_irq(transport, None)
    }

    /// Creates a new driver instance with an optional completion IRQ.
    pub fn try_new_with_irq(transport: T, irq: Option<usize>) -> DevResult<Self> {
        Ok(Self {
            inner: InnerDev::new(transport).map_err(as_dev_err)?,
            pending: [const { None }; MAX_PENDING_REQUESTS],
            irq,
        })
    }

    fn reserve_request(
        &mut self,
        id: BlockRequestId,
        op: PendingOp,
        buffer: NonNull<[u8]>,
    ) -> DevResult<usize> {
        let slot = self
            .pending
            .iter()
            .position(Option::is_none)
            .ok_or(axdriver_base::DevError::Again)?;
        self.pending[slot] = Some(PendingRequest::new(id, op, buffer));
        Ok(slot)
    }

    unsafe fn submit(
        &mut self,
        id: BlockRequestId,
        op: PendingOp,
        block_id: u64,
        buffer: NonNull<[u8]>,
    ) -> DevResult<BlockRequest> {
        let slot = self.reserve_request(id, op, buffer)?;
        let (inner, pending) = (&mut self.inner, &mut self.pending);
        let pending = pending[slot].as_mut().unwrap();
        let result = match op {
            PendingOp::Read => unsafe {
                inner.read_blocks_nb(
                    block_id as _,
                    &mut pending.request,
                    pending.buffer.as_mut(),
                    &mut pending.response,
                )
            },
            PendingOp::Write => unsafe {
                inner.write_blocks_nb(
                    block_id as _,
                    &mut pending.request,
                    pending.buffer.as_ref(),
                    &mut pending.response,
                )
            },
        };
        match result {
            Ok(token) => {
                pending.token = token;
                Ok(BlockRequest::Pending(id))
            }
            Err(err) => {
                self.pending[slot] = None;
                Err(as_dev_err(err))
            }
        }
    }
}

impl<H: Hal, T: Transport> BaseDriverOps for VirtIoBlkDev<H, T> {
    fn device_name(&self) -> &str {
        "virtio-blk"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn irq_num(&self) -> Option<usize> {
        // The LoongArch QEMU PCI probe derives a legacy INTx number from the
        // device slot, but the current platform does not reliably deliver
        // virtio-blk queue notifications through that route. Advertising it
        // makes the asynchronous completion task sleep forever after the
        // first request. Keep queue-based I/O enabled and let its completion
        // task poll the used ring while requests are outstanding.
        #[cfg(target_arch = "loongarch64")]
        {
            let _ = self.irq;
            None
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            self.irq
        }
    }
}

impl<H: Hal, T: Transport> BlockDriverOps for VirtIoBlkDev<H, T> {
    #[inline]
    fn num_blocks(&self) -> u64 {
        self.inner.capacity()
    }

    #[inline]
    fn block_size(&self) -> usize {
        virtio_drivers::device::blk::SECTOR_SIZE
    }

    fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> DevResult {
        self.inner
            .read_blocks(block_id as _, buf)
            .map_err(as_dev_err)
    }

    fn write_block(&mut self, block_id: u64, buf: &[u8]) -> DevResult {
        self.inner
            .write_blocks(block_id as _, buf)
            .map_err(as_dev_err)
    }

    fn flush(&mut self) -> DevResult {
        self.inner.flush().map_err(as_dev_err)
    }

    fn max_in_flight(&self) -> usize {
        self.inner.request_capacity().clamp(1, MAX_PENDING_REQUESTS)
    }

    fn supports_async(&self) -> bool {
        true
    }

    fn enable_interrupts(&mut self) {
        self.inner.enable_interrupts();
    }

    fn ack_interrupt(&mut self) -> bool {
        self.inner.ack_interrupt()
    }

    unsafe fn submit_read(
        &mut self,
        request_id: BlockRequestId,
        block_id: u64,
        buf: NonNull<[u8]>,
    ) -> DevResult<BlockRequest> {
        unsafe { self.submit(request_id, PendingOp::Read, block_id, buf) }
    }

    unsafe fn submit_write(
        &mut self,
        request_id: BlockRequestId,
        block_id: u64,
        buf: NonNull<[u8]>,
    ) -> DevResult<BlockRequest> {
        unsafe { self.submit(request_id, PendingOp::Write, block_id, buf) }
    }

    fn poll_complete(&mut self) -> Option<(BlockRequestId, DevResult)> {
        let token = self.inner.peek_used()?;
        let slot = self
            .pending
            .iter()
            .position(|pending| pending.as_ref().is_some_and(|it| it.token == token))?;

        let (inner, pending) = (&mut self.inner, &mut self.pending);
        let request = pending[slot].as_mut().unwrap();
        let result = match request.op {
            PendingOp::Read => unsafe {
                inner.complete_read_blocks(
                    token,
                    &request.request,
                    request.buffer.as_mut(),
                    &mut request.response,
                )
            },
            PendingOp::Write => unsafe {
                inner.complete_write_blocks(
                    token,
                    &request.request,
                    request.buffer.as_ref(),
                    &mut request.response,
                )
            },
        }
        .map_err(as_dev_err);
        let id = request.id;
        pending[slot] = None;
        Some((id, result))
    }
}
