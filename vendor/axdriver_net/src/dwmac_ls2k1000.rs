//! Polling Synopsys DWMAC 3.70a driver for Loongson 2K1000LA.
//!
//! The register programming and enhanced-descriptor layout are adapted from
//! the upstream StarryOS LS2K1000 driver.  Only the queue-facing layer is
//! different: this crate implements the older `NetDriverOps` API used by MOSS.
//! Polling is deliberate for the first hardware validation so DMA/MAC bring-up
//! remains independent from interrupt delivery.

use alloc::sync::Arc;
use core::{
    marker::PhantomData,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{AtomicBool, Ordering},
};

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use log::{info, warn};

use crate::{EthernetAddress, NetBuf, NetBufPool, NetBufPtr, NetDriverOps};

// Keep the upstream queue depth of 128. The buffers live in a static DMA
// section so the resulting 512 KiB allocation never lands on a kernel stack.
const RING_SIZE: usize = 128;
const DMA_BUFFER_LEN: usize = 2048;
const NET_BUFFER_LEN: usize = 1526;
const NET_BUFFER_COUNT: usize = 64;

const GMAC_CONTROL: usize = 0x0000;
const GMAC_FRAME_FILTER: usize = 0x0004;
const GMAC_MII_ADDR: usize = 0x0010;
const GMAC_MII_DATA: usize = 0x0014;
const GMAC_FLOW_CONTROL: usize = 0x0018;
const GMAC_VERSION: usize = 0x0020;
const GMAC_ADDR_HIGH0: usize = 0x0040;
const GMAC_ADDR_LOW0: usize = 0x0044;
const GMAC_RGSMII_STATUS: usize = 0x00d8;

const DMA_BUS_MODE: usize = 0x1000;
const DMA_TX_POLL_DEMAND: usize = 0x1004;
const DMA_RX_POLL_DEMAND: usize = 0x1008;
const DMA_RX_DESC_BASE: usize = 0x100c;
const DMA_TX_DESC_BASE: usize = 0x1010;
const DMA_STATUS: usize = 0x1014;
const DMA_CONTROL: usize = 0x1018;
const DMA_INTERRUPT_ENABLE: usize = 0x101c;
const DMA_AXI_BUS_MODE: usize = 0x1028;

const DMA_BUS_MODE_RESET: u32 = 1;
const DMA_BUS_MODE_BURST_LENGTH_32: u32 = 0x0000_2000;
const DMA_BUS_MODE_BURST_LENGTH_X8: u32 = 0x0100_0000;
const DMA_BUS_MODE_MIXED_BURST: u32 = 0x0400_0000;
const DMA_CONTROL_START_RX: u32 = 1 << 1;
const DMA_CONTROL_OPERATE_SECOND_FRAME: u32 = 1 << 2;
const DMA_CONTROL_START_TX: u32 = 1 << 13;
const DMA_CONTROL_FLOW_BITS: u32 = 0x00c0_1f00;
const DMA_CONTROL_STORE_AND_FORWARD: u32 = 0x0220_0000;

const GMAC_CONTROL_RX_ENABLE: u32 = 1 << 2;
const GMAC_CONTROL_TX_ENABLE: u32 = 1 << 3;
const GMAC_CONTROL_FULL_DUPLEX: u32 = 1 << 11;
const GMAC_CONTROL_FAST_ETHERNET_SPEED: u32 = 1 << 14;
const GMAC_CONTROL_MII_PORT: u32 = 1 << 15;
const GMAC_CONTROL_TX_CONFIG: u32 = 1 << 24;
const GMAC_CONTROL_CLEAR_BITS: u32 = 0x00f0_32f0;
const GMAC_FRAME_FILTER_CLEAR_BITS: u32 = 0x0000_02ff;
const GMAC_FRAME_FILTER_ENABLE: u32 = 0x8000_0000;
const GMAC_FLOW_CONTROL_PAUSE_MASK: u32 = 0xffff_0000;

const DESC_OWN: u32 = 1 << 31;
const RX_DESC_FIRST: u32 = 1 << 9;
const RX_DESC_LAST: u32 = 1 << 8;
const RX_DESC_ERROR: u32 = 1 << 15;
const RX_DESC_FRAME_LEN_MASK: u32 = 0x3fff << 16;
const RX_DESC_FRAME_LEN_SHIFT: u32 = 16;
const DESC_SIZE_MASK: u32 = 0x1fff;
const RX_DESC_END_RING: u32 = 1 << 15;
const TX_DESC_END_RING: u32 = 1 << 21;
const TX_DESC_FIRST: u32 = 1 << 28;
const TX_DESC_LAST: u32 = 1 << 29;
const TX_DESC_INTERRUPT: u32 = 1 << 30;

const MII_BUSY: u32 = 1;
const MII_CLOCK_RANGE_4: u32 = 1 << 4;
const MII_PHY_SHIFT: u32 = 11;
const MII_REG_SHIFT: u32 = 6;
const PHY_BMSR: u8 = 1;
const PHY_BMSR_LINK: u16 = 1 << 2;

/// Platform services required by the DWMAC driver.
pub trait DwmacHal {
    /// Translate a kernel virtual address to a DMA bus/physical address.
    fn virt_to_phys(virtual_address: usize) -> usize;

    /// Return a monotonically increasing time in milliseconds.
    fn current_ms() -> u64;

    /// Order CPU memory accesses against coherent DMA accesses.
    fn dma_barrier();
}

/// Values discovered from the selected Ethernet DT node.
#[derive(Clone, Copy, Debug)]
pub struct DwmacConfig {
    pub mac_address: Option<[u8; 6]>,
    pub phy_address: u8,
    pub irq: Option<usize>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DmaDesc {
    status: u32,
    length: u32,
    buffer1: u32,
    buffer2: u32,
}

const EMPTY_DESC: DmaDesc = DmaDesc {
    status: 0,
    length: 0,
    buffer1: 0,
    buffer2: 0,
};

#[repr(C, align(64))]
struct DescriptorRings {
    tx: [DmaDesc; RING_SIZE],
    rx: [DmaDesc; RING_SIZE],
}

#[repr(C, align(64))]
struct DmaBuffers {
    tx: [[u8; DMA_BUFFER_LEN]; RING_SIZE],
    rx: [[u8; DMA_BUFFER_LEN]; RING_SIZE],
}

static DMA_STORAGE_TAKEN: AtomicBool = AtomicBool::new(false);

#[unsafe(link_section = ".dma_uncached")]
static mut DMA_RINGS: DescriptorRings = DescriptorRings {
    tx: [EMPTY_DESC; RING_SIZE],
    rx: [EMPTY_DESC; RING_SIZE],
};

#[unsafe(link_section = ".dma_uncached")]
static mut DMA_BUFFERS: DmaBuffers = DmaBuffers {
    tx: [[0; DMA_BUFFER_LEN]; RING_SIZE],
    rx: [[0; DMA_BUFFER_LEN]; RING_SIZE],
};

/// Integrated LS2K1000LA GMAC0 network device.
pub struct DwmacLs2k1000<H: DwmacHal> {
    base: *mut u8,
    mac: [u8; 6],
    phy_address: u8,
    irq: Option<usize>,
    rings: &'static mut DescriptorRings,
    buffers: &'static mut DmaBuffers,
    rx_index: usize,
    tx_index: usize,
    net_pool: Arc<NetBufPool>,
    _hal: PhantomData<H>,
}

unsafe impl<H: DwmacHal> Send for DwmacLs2k1000<H> {}
unsafe impl<H: DwmacHal> Sync for DwmacLs2k1000<H> {}

impl<H: DwmacHal> DwmacLs2k1000<H> {
    /// Create and initialize GMAC0 at the supplied MMIO virtual address.
    ///
    /// # Safety
    ///
    /// `base` must map an LS2K1000LA DWMAC 3.70a register window and must be
    /// exclusively owned by this driver.
    pub unsafe fn try_new(base: usize, config: DwmacConfig) -> DevResult<Self> {
        if DMA_STORAGE_TAKEN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DevError::ResourceBusy);
        }

        let mut dev = Self {
            base: base as *mut u8,
            mac: [0; 6],
            phy_address: config.phy_address,
            irq: config.irq,
            rings: unsafe { &mut *core::ptr::addr_of_mut!(DMA_RINGS) },
            buffers: unsafe { &mut *core::ptr::addr_of_mut!(DMA_BUFFERS) },
            rx_index: 0,
            tx_index: 0,
            net_pool: NetBufPool::new(NET_BUFFER_COUNT, NET_BUFFER_LEN)?,
            _hal: PhantomData,
        };

        dev.mac = config.mac_address.unwrap_or_else(|| dev.read_mac_address());
        if !Self::valid_mac(dev.mac) {
            // Locally administered, deterministic address for boards whose
            // firmware did not leave a station address in the MAC registers.
            dev.mac = [0x02, 0x4d, 0x4f, 0x53, 0x53, 0x00];
            dev.write_mac_address(dev.mac);
            warn!("LS2K1000 GMAC0 had no valid firmware MAC; using 02:4d:4f:53:53:00");
        }

        dev.stop_tx_rx();
        dev.reset_dma()?;
        dev.initialize_descriptors()?;
        dev.initialize_mac();

        let phy_status = dev
            .mdio_read(dev.phy_address, PHY_BMSR)
            .unwrap_or(0xffff);
        let rgmii = dev.read_reg(GMAC_RGSMII_STATUS);
        info!(
            "LS2K1000 GMAC0 initialized (StarryOS hardware sequence, polling): version={:#010x}, mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, phy0_bmsr={:#06x}, rgmii={:#010x}, link={}",
            dev.read_reg(GMAC_VERSION),
            dev.mac[0],
            dev.mac[1],
            dev.mac[2],
            dev.mac[3],
            dev.mac[4],
            dev.mac[5],
            phy_status,
            rgmii,
            phy_status & PHY_BMSR_LINK != 0,
        );
        Ok(dev)
    }

    fn initialize_descriptors(&mut self) -> DevResult {
        for index in 0..RING_SIZE {
            let ring_end = index + 1 == RING_SIZE;
            let rx_buffer_pa = Self::dma_address(self.buffers.rx[index].as_ptr() as usize)?;
            let tx_buffer_pa = Self::dma_address(self.buffers.tx[index].as_ptr() as usize)?;
            self.rings.rx[index] = DmaDesc {
                status: DESC_OWN,
                length: (DMA_BUFFER_LEN as u32 & DESC_SIZE_MASK)
                    | if ring_end { RX_DESC_END_RING } else { 0 },
                buffer1: rx_buffer_pa,
                buffer2: 0,
            };
            self.rings.tx[index] = DmaDesc {
                status: if ring_end { TX_DESC_END_RING } else { 0 },
                length: 0,
                buffer1: tx_buffer_pa,
                buffer2: 0,
            };
        }

        H::dma_barrier();
        let rx_desc_pa = Self::dma_address(self.rings.rx.as_ptr() as usize)?;
        let tx_desc_pa = Self::dma_address(self.rings.tx.as_ptr() as usize)?;
        self.write_reg(DMA_RX_DESC_BASE, rx_desc_pa);
        self.write_reg(DMA_TX_DESC_BASE, tx_desc_pa);
        Ok(())
    }

    fn initialize_mac(&mut self) {
        self.write_mac_address(self.mac);
        self.write_reg(DMA_INTERRUPT_ENABLE, 0);
        self.write_reg(DMA_STATUS, u32::MAX);
        self.write_reg(
            DMA_BUS_MODE,
            DMA_BUS_MODE_MIXED_BURST | DMA_BUS_MODE_BURST_LENGTH_X8 | DMA_BUS_MODE_BURST_LENGTH_32,
        );
        self.write_reg(DMA_AXI_BUS_MODE, 0xff | (0x77 << 16));

        let mut mac_config = self.read_reg(GMAC_CONTROL) | GMAC_CONTROL_TX_CONFIG;
        mac_config &= !GMAC_CONTROL_CLEAR_BITS;
        mac_config |= GMAC_CONTROL_FULL_DUPLEX;
        self.write_reg(GMAC_CONTROL, mac_config);

        let mut frame_filter = self.read_reg(GMAC_FRAME_FILTER);
        frame_filter &= !GMAC_FRAME_FILTER_CLEAR_BITS;
        frame_filter |= GMAC_FRAME_FILTER_ENABLE;
        self.write_reg(GMAC_FRAME_FILTER, frame_filter);

        let dma_control = (self.read_reg(DMA_CONTROL) & !DMA_CONTROL_FLOW_BITS)
            | DMA_CONTROL_STORE_AND_FORWARD
            | DMA_CONTROL_OPERATE_SECOND_FRAME;
        self.write_reg(DMA_CONTROL, dma_control);
        self.write_reg(GMAC_FLOW_CONTROL, GMAC_FLOW_CONTROL_PAUSE_MASK);

        let (speed_bits, duplex) = self.rgmii_mode();
        let link_config = (self.read_reg(GMAC_CONTROL)
            & !(GMAC_CONTROL_MII_PORT
                | GMAC_CONTROL_FAST_ETHERNET_SPEED
                | GMAC_CONTROL_FULL_DUPLEX))
            | speed_bits
            | duplex
            | GMAC_CONTROL_TX_ENABLE
            | GMAC_CONTROL_RX_ENABLE;
        self.write_reg(GMAC_CONTROL, link_config);
        self.write_reg(
            DMA_CONTROL,
            self.read_reg(DMA_CONTROL) | DMA_CONTROL_START_TX | DMA_CONTROL_START_RX,
        );
        H::dma_barrier();
        self.write_reg(DMA_RX_POLL_DEMAND, 0);
    }

    fn stop_tx_rx(&self) {
        self.write_reg(
            DMA_CONTROL,
            self.read_reg(DMA_CONTROL) & !(DMA_CONTROL_START_TX | DMA_CONTROL_START_RX),
        );
        self.write_reg(
            GMAC_CONTROL,
            self.read_reg(GMAC_CONTROL) & !(GMAC_CONTROL_TX_ENABLE | GMAC_CONTROL_RX_ENABLE),
        );
        H::dma_barrier();
    }

    fn reset_dma(&self) -> DevResult {
        self.write_reg(DMA_BUS_MODE, DMA_BUS_MODE_RESET);
        let deadline = H::current_ms().saturating_add(100);
        while self.read_reg(DMA_BUS_MODE) & DMA_BUS_MODE_RESET != 0 {
            if H::current_ms() >= deadline {
                warn!("LS2K1000 GMAC0 DMA reset timed out");
                return Err(DevError::BadState);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    fn receive_one(&mut self) -> DevResult<NetBufPtr> {
        for _ in 0..RING_SIZE {
            let index = self.rx_index;
            H::dma_barrier();
            let status = unsafe { read_volatile(&self.rings.rx[index].status) };
            if status & DESC_OWN != 0 {
                return Err(DevError::Again);
            }

            self.rx_index = (index + 1) % RING_SIZE;
            let raw_len = ((status & RX_DESC_FRAME_LEN_MASK) >> RX_DESC_FRAME_LEN_SHIFT) as usize;
            let valid = status & (RX_DESC_ERROR | RX_DESC_FIRST | RX_DESC_LAST)
                == (RX_DESC_FIRST | RX_DESC_LAST)
                && (1..=NET_BUFFER_LEN).contains(&raw_len);

            let result = if valid {
                // Match the upstream StarryOS driver: the enhanced descriptor
                // reports the length consumed by its network queue directly.
                let packet_len = raw_len;
                let mut packet = self.net_pool.alloc_boxed().ok_or(DevError::NoMemory)?;
                packet.set_packet_len(packet_len);
                packet
                    .packet_mut()
                    .copy_from_slice(&self.buffers.rx[index][..packet_len]);
                Some(packet.into_buf_ptr())
            } else {
                warn!("LS2K1000 GMAC0 dropped RX descriptor {index}: status={status:#010x}");
                None
            };

            H::dma_barrier();
            unsafe { write_volatile(&mut self.rings.rx[index].status, DESC_OWN) };
            H::dma_barrier();
            let dma_status = self.read_reg(DMA_STATUS);
            self.write_reg(DMA_STATUS, dma_status);
            self.write_reg(DMA_RX_POLL_DEMAND, 0);
            if let Some(packet) = result {
                return Ok(packet);
            }
        }
        Err(DevError::Again)
    }

    fn transmit_one(&mut self, tx_buf: NetBufPtr) -> DevResult {
        let tx_buf = unsafe { NetBuf::from_buf_ptr(tx_buf) };
        let len = tx_buf.packet().len();
        if !(1..=NET_BUFFER_LEN).contains(&len) {
            return Err(DevError::InvalidParam);
        }
        let index = self.tx_index;
        if unsafe { read_volatile(&self.rings.tx[index].status) } & DESC_OWN != 0 {
            return Err(DevError::Again);
        }

        self.buffers.tx[index][..len].copy_from_slice(tx_buf.packet());
        let ring_end = index + 1 == RING_SIZE;
        unsafe {
            write_volatile(
                &mut self.rings.tx[index].length,
                len as u32 & DESC_SIZE_MASK,
            );
            write_volatile(
                &mut self.rings.tx[index].status,
                (if ring_end { TX_DESC_END_RING } else { 0 })
                    | TX_DESC_INTERRUPT
                    | TX_DESC_FIRST
                    | TX_DESC_LAST,
            );
        }
        H::dma_barrier();
        unsafe {
            let status = read_volatile(&self.rings.tx[index].status);
            write_volatile(&mut self.rings.tx[index].status, status | DESC_OWN);
        }
        H::dma_barrier();
        self.write_reg(DMA_TX_POLL_DEMAND, 0);

        // Synchronous completion lets ownership of the stack-provided buffer
        // return immediately and keeps the initial driver simple.
        let deadline = H::current_ms().saturating_add(100);
        while unsafe { read_volatile(&self.rings.tx[index].status) } & DESC_OWN != 0 {
            if H::current_ms() >= deadline {
                let dma_status = self.read_reg(DMA_STATUS);
                warn!(
                    "LS2K1000 GMAC0 TX timed out: desc={:#010x}, dma_status={:#010x}",
                    unsafe { read_volatile(&self.rings.tx[index].status) },
                    dma_status
                );
                return Err(DevError::Io);
            }
            core::hint::spin_loop();
        }
        H::dma_barrier();
        let completed_status = unsafe { read_volatile(&self.rings.tx[index].status) };
        unsafe {
            write_volatile(
                &mut self.rings.tx[index].status,
                if ring_end { TX_DESC_END_RING } else { 0 },
            );
            write_volatile(&mut self.rings.tx[index].length, 0);
        }
        let dma_status = self.read_reg(DMA_STATUS);
        self.write_reg(DMA_STATUS, dma_status);
        self.tx_index = (index + 1) % RING_SIZE;
        if completed_status & RX_DESC_ERROR != 0 {
            warn!("LS2K1000 GMAC0 TX descriptor error {index}: status={completed_status:#010x}");
            return Err(DevError::Io);
        }
        Ok(())
    }

    fn rgmii_mode(&self) -> (u32, u32) {
        let status = self.read_reg(GMAC_RGSMII_STATUS);
        if status == 0 {
            // Some LS2K1000 integrations do not expose PCS status for RGMII.
            // The board firmware negotiates this port at 1000/full, so keep
            // GMII mode rather than interpreting an all-zero register as 10M.
            return (0, GMAC_CONTROL_FULL_DUPLEX);
        }
        let duplex = if status & 1 != 0 || status == 0 {
            GMAC_CONTROL_FULL_DUPLEX
        } else {
            0
        };
        let speed = match (status >> 1) & 0x3 {
            0 => GMAC_CONTROL_MII_PORT, // 10 Mbit/s
            1 => GMAC_CONTROL_MII_PORT | GMAC_CONTROL_FAST_ETHERNET_SPEED,
            _ => 0, // 1000 Mbit/s GMII/RGMII
        };
        (speed, duplex)
    }

    fn mdio_read(&self, phy: u8, register: u8) -> Option<u16> {
        let deadline = H::current_ms().saturating_add(10);
        while self.read_reg(GMAC_MII_ADDR) & MII_BUSY != 0 {
            if H::current_ms() >= deadline {
                return None;
            }
        }
        self.write_reg(
            GMAC_MII_ADDR,
            ((phy as u32) << MII_PHY_SHIFT)
                | ((register as u32) << MII_REG_SHIFT)
                | MII_CLOCK_RANGE_4
                | MII_BUSY,
        );
        let deadline = H::current_ms().saturating_add(10);
        while self.read_reg(GMAC_MII_ADDR) & MII_BUSY != 0 {
            if H::current_ms() >= deadline {
                return None;
            }
        }
        Some(self.read_reg(GMAC_MII_DATA) as u16)
    }

    fn dma_address(virtual_address: usize) -> DevResult<u32> {
        H::virt_to_phys(virtual_address)
            .try_into()
            .map_err(|_| DevError::NoMemory)
    }

    fn valid_mac(mac: [u8; 6]) -> bool {
        mac != [0; 6] && mac != [0xff; 6] && mac[0] & 1 == 0
    }

    fn read_mac_address(&self) -> [u8; 6] {
        let low = self.read_reg(GMAC_ADDR_LOW0);
        let high = self.read_reg(GMAC_ADDR_HIGH0);
        [
            low as u8,
            (low >> 8) as u8,
            (low >> 16) as u8,
            (low >> 24) as u8,
            high as u8,
            (high >> 8) as u8,
        ]
    }

    fn write_mac_address(&self, mac: [u8; 6]) {
        let low = u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]);
        let high = u16::from_le_bytes([mac[4], mac[5]]) as u32;
        self.write_reg(GMAC_ADDR_HIGH0, high);
        self.write_reg(GMAC_ADDR_LOW0, low);
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { self.base.add(offset).cast::<u32>().read_volatile() }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        unsafe { self.base.add(offset).cast::<u32>().write_volatile(value) }
    }
}

impl<H: DwmacHal> BaseDriverOps for DwmacLs2k1000<H> {
    fn device_name(&self) -> &str {
        "ls2k1000-gmac0"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }

    fn irq_num(&self) -> Option<usize> {
        // The current queue implementation is polling. Keep the parsed IRQ in
        // the device state, but do not advertise it until DMA IRQ acknowledge
        // and recovery are wired to NetDriverOps.
        let _ = self.irq;
        None
    }
}

impl<H: DwmacHal> NetDriverOps for DwmacLs2k1000<H> {
    fn mac_address(&self) -> EthernetAddress {
        EthernetAddress(self.mac)
    }

    fn can_transmit(&self) -> bool {
        (unsafe { read_volatile(&self.rings.tx[self.tx_index].status) } & DESC_OWN) == 0
    }

    fn can_receive(&self) -> bool {
        H::dma_barrier();
        (unsafe { read_volatile(&self.rings.rx[self.rx_index].status) } & DESC_OWN) == 0
    }

    fn rx_queue_size(&self) -> usize {
        RING_SIZE
    }

    fn tx_queue_size(&self) -> usize {
        RING_SIZE
    }

    fn recycle_rx_buffer(&mut self, rx_buf: NetBufPtr) -> DevResult {
        drop(unsafe { NetBuf::from_buf_ptr(rx_buf) });
        Ok(())
    }

    fn recycle_tx_buffers(&mut self) -> DevResult {
        Ok(())
    }

    fn transmit(&mut self, tx_buf: NetBufPtr) -> DevResult {
        self.transmit_one(tx_buf)
    }

    fn receive(&mut self) -> DevResult<NetBufPtr> {
        self.receive_one()
    }

    fn alloc_tx_buffer(&mut self, size: usize) -> DevResult<NetBufPtr> {
        if size > NET_BUFFER_LEN {
            return Err(DevError::InvalidParam);
        }
        let mut buffer = self.net_pool.alloc_boxed().ok_or(DevError::NoMemory)?;
        buffer.set_packet_len(size);
        Ok(buffer.into_buf_ptr())
    }
}
