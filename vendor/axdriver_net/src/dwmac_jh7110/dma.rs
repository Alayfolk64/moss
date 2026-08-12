//! Statically allocated non-coherent DMA rings and packet buffers.

use core::{
    mem::size_of,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{AtomicBool, Ordering},
};

use axdriver_base::{DevError, DevResult};

pub(super) const RING_SIZE: usize = 64;
pub(super) const DMA_BUFFER_LEN: usize = 2048;
const BASIC_DESCRIPTOR_LEN: usize = 16;
const JH7110_AXI_BUS_WIDTH: usize = 8;

pub(super) const DESC3_OWN: u32 = 1 << 31;
pub(super) const DESC3_IOC_OR_CONTEXT: u32 = 1 << 30;
pub(super) const DESC3_FD: u32 = 1 << 29;
pub(super) const DESC3_LD: u32 = 1 << 28;
pub(super) const RX_DESC3_BUF1V: u32 = 1 << 24;
/// Error summary is in bit 15 for both normal RX and normal TX write-back.
pub(super) const DESC3_ERROR: u32 = 1 << 15;
pub(super) const RX_DESC3_LEN_MASK: u32 = 0x7fff;
const TX_DESC2_IOC: u32 = 1 << 31;
const TX_DESC2_LEN_MASK: u32 = 0x3fff;

#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub(super) struct DmaDesc {
    des0: u32,
    des1: u32,
    des2: u32,
    des3: u32,
    padding: [u32; 12],
}

const EMPTY_DESC: DmaDesc = DmaDesc {
    des0: 0,
    des1: 0,
    des2: 0,
    des3: 0,
    padding: [0; 12],
};

impl DmaDesc {
    pub(super) const fn size() -> usize {
        size_of::<Self>()
    }

    /// Number of AXI-width words skipped after each 16-byte EQoS descriptor.
    ///
    /// One descriptor occupies a complete 64-byte cache line so invalidating a
    /// DMA-updated descriptor cannot discard CPU changes to its neighbour. The
    /// DWMAC DSL field is measured in bus widths (8 bytes on JH7110), not in
    /// bytes or 32-bit words: `(64 - 16) / 8 == 6`.
    pub(super) const fn skip_length() -> u32 {
        ((Self::size() - BASIC_DESCRIPTOR_LEN) / JH7110_AXI_BUS_WIDTH) as u32
    }

    pub(super) fn status(&self) -> u32 {
        unsafe { read_volatile(&self.des3) }
    }

    pub(super) fn clear(&mut self) {
        *self = EMPTY_DESC;
    }

    pub(super) fn arm_rx(&mut self, buffer_address: usize) {
        self.des0 = buffer_address as u32;
        self.des1 = (buffer_address >> 32) as u32;
        self.des2 = 0;
        unsafe {
            write_volatile(
                &mut self.des3,
                DESC3_OWN | DESC3_IOC_OR_CONTEXT | RX_DESC3_BUF1V,
            );
        }
    }

    pub(super) fn arm_tx(&mut self, buffer_address: usize, length: usize) {
        self.des0 = buffer_address as u32;
        self.des1 = (buffer_address >> 32) as u32;
        self.des2 = TX_DESC2_IOC | (length as u32 & TX_DESC2_LEN_MASK);
        unsafe {
            write_volatile(
                &mut self.des3,
                DESC3_OWN | DESC3_FD | DESC3_LD | length as u32,
            );
        }
    }
}

#[repr(C, align(64))]
pub(super) struct DescriptorRings {
    pub(super) tx: [DmaDesc; RING_SIZE],
    pub(super) rx: [DmaDesc; RING_SIZE],
}

#[repr(C, align(64))]
pub(super) struct DmaBuffers {
    pub(super) tx: [[u8; DMA_BUFFER_LEN]; RING_SIZE],
    pub(super) rx: [[u8; DMA_BUFFER_LEN]; RING_SIZE],
}

static STORAGE_TAKEN: AtomicBool = AtomicBool::new(false);

#[unsafe(link_section = ".dma_uncached")]
static mut RINGS: DescriptorRings = DescriptorRings {
    tx: [EMPTY_DESC; RING_SIZE],
    rx: [EMPTY_DESC; RING_SIZE],
};

#[unsafe(link_section = ".dma_uncached")]
static mut BUFFERS: DmaBuffers = DmaBuffers {
    tx: [[0; DMA_BUFFER_LEN]; RING_SIZE],
    rx: [[0; DMA_BUFFER_LEN]; RING_SIZE],
};

pub(super) struct DmaStorage {
    pub(super) rings: &'static mut DescriptorRings,
    pub(super) buffers: &'static mut DmaBuffers,
}

impl DmaStorage {
    pub(super) fn take() -> DevResult<Self> {
        if STORAGE_TAKEN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DevError::ResourceBusy);
        }

        Ok(Self {
            rings: unsafe { &mut *core::ptr::addr_of_mut!(RINGS) },
            buffers: unsafe { &mut *core::ptr::addr_of_mut!(BUFFERS) },
        })
    }
}

impl Drop for DmaStorage {
    fn drop(&mut self) {
        STORAGE_TAKEN.store(false, Ordering::Release);
    }
}
