//! Common traits and types for block storage device drivers (i.e. disk).

#![no_std]
#![cfg_attr(doc, feature(doc_cfg))]

use core::ptr::NonNull;

#[cfg(any(feature = "ramdisk", feature = "ramdisk-static"))]
mod block_range;

#[cfg(feature = "bcm2835-sdhci")]
pub mod bcm2835sdhci;

#[cfg(feature = "ramdisk")]
pub mod ramdisk;

#[cfg(feature = "ramdisk-static")]
pub mod ramdisk_static;

#[cfg(feature = "ahci")]
pub mod ahci;
#[cfg(feature = "sdmmc")]
pub mod sdmmc;

#[doc(no_inline)]
pub use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};

/// Identifier returned for an asynchronous block request.
pub type BlockRequestId = u64;

/// Result of submitting a block request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockRequest {
    /// The driver completed the request before returning from `submit_*`.
    Complete,
    /// The request is owned by the driver until `poll_complete` reports this ID.
    Pending(BlockRequestId),
}

/// Operations that require a block storage device driver to implement.
pub trait BlockDriverOps: BaseDriverOps {
    /// The number of blocks in this storage device.
    ///
    /// The total size of the device is `num_blocks() * block_size()`.
    fn num_blocks(&self) -> u64;
    /// The size of each block in bytes.
    fn block_size(&self) -> usize;

    /// Reads blocked data from the given block.
    ///
    /// The size of the buffer may exceed the block size, in which case multiple
    /// contiguous blocks will be read.
    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> DevResult;

    /// Writes blocked data to the given block.
    ///
    /// The size of the buffer may exceed the block size, in which case multiple
    /// contiguous blocks will be written.
    fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult;

    /// Flushes the device to write all pending data to the storage.
    fn flush(&self) -> DevResult;

    /// Maximum number of requests which may be in flight at once.
    fn max_in_flight(&self) -> usize {
        1
    }

    /// Whether this driver can return [`BlockRequest::Pending`].
    fn supports_async(&self) -> bool {
        false
    }

    /// Enables completion interrupts in the device.
    fn enable_interrupts(&self) {}

    /// Acknowledges a pending device interrupt.
    fn ack_interrupt(&self) -> bool {
        false
    }

    /// Submits a read without waiting for completion.
    ///
    /// # Safety
    ///
    /// For a pending request, `buf` must remain valid, pinned and inaccessible
    /// to CPU users until [`BlockDriverOps::poll_complete`] returns its ID.
    unsafe fn submit_read(
        &self,
        request_id: BlockRequestId,
        block_id: u64,
        mut buf: NonNull<[u8]>,
    ) -> DevResult<BlockRequest> {
        let _ = request_id;
        self.read_block(block_id, unsafe { buf.as_mut() })?;
        Ok(BlockRequest::Complete)
    }

    /// Submits a write without waiting for completion.
    ///
    /// # Safety
    ///
    /// For a pending request, `buf` must remain valid, pinned and immutable
    /// until [`BlockDriverOps::poll_complete`] returns its ID.
    unsafe fn submit_write(
        &self,
        request_id: BlockRequestId,
        block_id: u64,
        buf: NonNull<[u8]>,
    ) -> DevResult<BlockRequest> {
        let _ = request_id;
        self.write_block(block_id, unsafe { buf.as_ref() })?;
        Ok(BlockRequest::Complete)
    }

    /// Completes the next used asynchronous request, if one is available.
    fn poll_complete(&self) -> Option<(BlockRequestId, DevResult)> {
        None
    }
}
