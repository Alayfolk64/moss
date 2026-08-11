//! Synopsys DWMAC 5.20 (EQoS) driver for StarFive JH7110 GMAC1.
//!
//! The register layout and descriptor programming follow the upstream
//! U-Boot EQoS driver. JH7110 DMA is non-coherent, so the platform HAL must
//! clean/invalidate every descriptor and packet buffer ownership transfer.

use alloc::sync::Arc;
use core::{
    marker::PhantomData,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use log::{info, warn};

use crate::{EthernetAddress, NetBuf, NetBufPool, NetBufPtr, NetDriverOps};

mod dma;
mod registers;

use self::{
    dma::{
        DMA_BUFFER_LEN, DESC3_FD, DESC3_LD, DESC3_OWN, DmaDesc, DmaStorage, RING_SIZE,
        RX_DESC3_ERROR, RX_DESC3_LEN_MASK,
    },
    registers::*,
};

const NET_BUFFER_LEN: usize = 1526;
const NET_BUFFER_COUNT: usize = 64;

/// Platform services required by the JH7110 EQoS driver.
pub trait Jh7110Hal {
    fn virt_to_phys(virtual_address: usize) -> usize;
    fn current_ms() -> u64;
    fn dma_clean(virtual_address: usize, size: usize);
    fn dma_invalidate(virtual_address: usize, size: usize);
    fn dma_barrier();
}

#[derive(Clone, Copy, Debug)]
pub struct Jh7110Config {
    pub mac_address: Option<[u8; 6]>,
    pub phy_address: u8,
    pub irq: Option<usize>,
}

static DMA_MMIO_BASE: AtomicUsize = AtomicUsize::new(0);
static DMA_IRQ_ACTIVE: AtomicBool = AtomicBool::new(false);
static DMA_IRQ_EVENTS: AtomicU32 = AtomicU32::new(0);

pub struct DwmacJh7110<H: Jh7110Hal> {
    base: *mut u8,
    mac: [u8; 6],
    phy_address: u8,
    irq: Option<usize>,
    storage: DmaStorage,
    rx_index: usize,
    tx_submit: usize,
    tx_clean: usize,
    tx_in_flight: usize,
    net_pool: Arc<NetBufPool>,
    _hal: PhantomData<H>,
}

unsafe impl<H: Jh7110Hal> Send for DwmacJh7110<H> {}
unsafe impl<H: Jh7110Hal> Sync for DwmacJh7110<H> {}

impl<H: Jh7110Hal> DwmacJh7110<H> {
    /// Initializes GMAC1. Platform glue must already have enabled the JH7110
    /// clock/reset domain and configured the external RGMII PHY.
    ///
    /// # Safety
    /// `base` must exclusively map a JH7110 DWMAC 5.20 register window.
    pub unsafe fn try_new(base: usize, config: Jh7110Config) -> DevResult<Self> {
        let mut dev = Self {
            base: base as *mut u8,
            mac: [0; 6],
            phy_address: config.phy_address,
            irq: config.irq,
            storage: DmaStorage::take()?,
            rx_index: 0,
            tx_submit: 0,
            tx_clean: 0,
            tx_in_flight: 0,
            net_pool: NetBufPool::new(NET_BUFFER_COUNT, NET_BUFFER_LEN)?,
            _hal: PhantomData,
        };

        dev.mac = config.mac_address.unwrap_or_else(|| dev.read_mac_address());
        if !Self::valid_mac(dev.mac) {
            dev.mac = [0x02, 0x4d, 0x4f, 0x53, 0x53, 0x01];
            info!("JH7110 GMAC1 had no firmware MAC; using 02:4d:4f:53:53:01");
        }

        dev.stop_dma();
        dev.reset_dma()?;
        dev.initialize_mtl_mac();
        dev.initialize_descriptors()?;
        dev.start_dma();
        DMA_MMIO_BASE.store(base, Ordering::Release);

        let bmsr = dev.mdio_read(dev.phy_address, PHY_BMSR).unwrap_or(0xffff);
        info!(
            "JH7110 GMAC1 initialized: version={:#010x}, hw_feature1={:#010x}, \
             mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, phy_bmsr={:#06x}, link={}",
            dev.read_reg(MAC_VERSION),
            dev.read_reg(MAC_HW_FEATURE1),
            dev.mac[0],
            dev.mac[1],
            dev.mac[2],
            dev.mac[3],
            dev.mac[4],
            dev.mac[5],
            bmsr,
            bmsr & PHY_BMSR_LINK != 0,
        );
        Ok(dev)
    }

    pub fn enable_interrupt_mode(&mut self) {
        if self.irq.is_none() {
            return;
        }
        self.write_reg(DMA_CH0_STATUS, u32::MAX);
        DMA_IRQ_EVENTS.store(0, Ordering::Release);
        DMA_IRQ_ACTIVE.store(true, Ordering::Release);
        self.write_reg(DMA_CH0_INTERRUPT_ENABLE, DMA_IRQ_MASK);
        H::dma_barrier();
    }

    fn reset_dma(&self) -> DevResult {
        self.write_reg(DMA_MODE, DMA_MODE_SWR);
        for _ in 0..4 {
            H::dma_barrier();
        }
        let deadline = H::current_ms().saturating_add(100);
        while self.read_reg(DMA_MODE) & DMA_MODE_SWR != 0 {
            if H::current_ms() >= deadline {
                warn!("JH7110 GMAC1 DMA reset timed out; clock/reset domain is not active");
                return Err(DevError::BadState);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    fn initialize_mtl_mac(&self) {
        self.write_reg(MTL_TXQ0_OPERATION_MODE, (7 << 16) | (2 << 2) | (1 << 1));
        self.write_reg(MTL_TXQ0_QUANTUM_WEIGHT, 0x10);
        self.write_reg(MTL_RXQ0_OPERATION_MODE, (7 << 20) | (1 << 5));

        self.write_reg(MAC_RXQ_CTRL0, (self.read_reg(MAC_RXQ_CTRL0) & !3) | 2);
        self.write_reg(MAC_RXQ_CTRL1, self.read_reg(MAC_RXQ_CTRL1) | (1 << 20));
        self.write_reg(
            MAC_PACKET_FILTER,
            self.read_reg(MAC_PACKET_FILTER) | MAC_PACKET_FILTER_PR,
        );
        self.write_reg(MAC_Q0_TX_FLOW_CTRL, 0xffff_0002);
        self.write_reg(MAC_RX_FLOW_CTRL, self.read_reg(MAC_RX_FLOW_CTRL) | 1);
        self.write_mac_address(self.mac);

        // The VF2 RGMII PHY is negotiated by U-Boot before the kernel jump.
        // Select 1000/full; PS/FES are changed later when PHY speed control is added.
        let config =
            (self.read_reg(MAC_CONFIGURATION) & !(MAC_CONFIG_PS | MAC_CONFIG_FES)) | MAC_CONFIG_DM;
        self.write_reg(MAC_CONFIGURATION, config);

        self.write_reg(DMA_MODE, self.read_reg(DMA_MODE) | DMA_MODE_DCHE);
        self.write_reg(
            DMA_SYSBUS_MODE,
            (2 << 16) | DMA_SYSBUS_EAME | DMA_SYSBUS_BLEN16 | DMA_SYSBUS_BLEN8 | DMA_SYSBUS_BLEN4,
        );
        self.write_reg(
            DMA_CH0_CONTROL,
            DMA_CH_CONTROL_PBLX8 | DMA_CH_CONTROL_DSL_64,
        );
        self.write_reg(
            DMA_CH0_TX_CONTROL,
            DMA_TX_CONTROL_TXPBL_16 | DMA_TX_CONTROL_OSP,
        );
        self.write_reg(
            DMA_CH0_RX_CONTROL,
            DMA_RX_CONTROL_RXPBL_8 | ((DMA_BUFFER_LEN as u32) << 1),
        );
        self.write_reg(DMA_CH0_INTERRUPT_ENABLE, 0);
        self.write_reg(DMA_CH0_STATUS, u32::MAX);
    }

    fn initialize_descriptors(&mut self) -> DevResult {
        for index in 0..RING_SIZE {
            let rx_pa =
                Self::dma_address(self.storage.buffers.rx[index].as_ptr() as usize)?;
            self.storage.rings.rx[index].arm_rx(rx_pa);
            self.storage.rings.tx[index].clear();
            H::dma_clean(
                &self.storage.rings.rx[index] as *const DmaDesc as usize,
                DmaDesc::size(),
            );
            H::dma_clean(
                &self.storage.rings.tx[index] as *const DmaDesc as usize,
                DmaDesc::size(),
            );
            H::dma_invalidate(
                self.storage.buffers.rx[index].as_ptr() as usize,
                DMA_BUFFER_LEN,
            );
        }
        H::dma_barrier();

        let tx_pa = Self::dma_address(self.storage.rings.tx.as_ptr() as usize)?;
        let rx_pa = Self::dma_address(self.storage.rings.rx.as_ptr() as usize)?;
        self.write_reg(DMA_CH0_TXDESC_LIST_HI, (tx_pa >> 32) as u32);
        self.write_reg(DMA_CH0_TXDESC_LIST, tx_pa as u32);
        self.write_reg(DMA_CH0_TXDESC_RING_LEN, RING_SIZE as u32 - 1);
        self.write_reg(DMA_CH0_RXDESC_LIST_HI, (rx_pa >> 32) as u32);
        self.write_reg(DMA_CH0_RXDESC_LIST, rx_pa as u32);
        self.write_reg(DMA_CH0_RXDESC_RING_LEN, RING_SIZE as u32 - 1);
        self.write_reg(DMA_CH0_TXDESC_TAIL, tx_pa as u32);
        self.write_reg(
            DMA_CH0_RXDESC_TAIL,
            (rx_pa + (RING_SIZE - 1) * DmaDesc::size()) as u32,
        );
        Ok(())
    }

    fn start_dma(&self) {
        self.write_reg(
            DMA_CH0_TX_CONTROL,
            self.read_reg(DMA_CH0_TX_CONTROL) | DMA_TX_CONTROL_ST,
        );
        self.write_reg(
            DMA_CH0_RX_CONTROL,
            self.read_reg(DMA_CH0_RX_CONTROL) | DMA_RX_CONTROL_SR,
        );
        self.write_reg(
            MAC_CONFIGURATION,
            self.read_reg(MAC_CONFIGURATION) | MAC_CONFIG_TE | MAC_CONFIG_RE,
        );
        H::dma_barrier();
    }

    fn stop_dma(&self) {
        self.write_reg(DMA_CH0_INTERRUPT_ENABLE, 0);
        self.write_reg(
            DMA_CH0_TX_CONTROL,
            self.read_reg(DMA_CH0_TX_CONTROL) & !DMA_TX_CONTROL_ST,
        );
        self.write_reg(
            DMA_CH0_RX_CONTROL,
            self.read_reg(DMA_CH0_RX_CONTROL) & !DMA_RX_CONTROL_SR,
        );
        self.write_reg(
            MAC_CONFIGURATION,
            self.read_reg(MAC_CONFIGURATION) & !(MAC_CONFIG_TE | MAC_CONFIG_RE),
        );
        H::dma_barrier();
    }

    fn receive_one(&mut self) -> DevResult<NetBufPtr> {
        let index = self.rx_index;
        let desc_addr = &self.storage.rings.rx[index] as *const DmaDesc as usize;
        H::dma_invalidate(desc_addr, DmaDesc::size());
        let status = self.storage.rings.rx[index].status();
        if status & DESC3_OWN != 0 {
            return Err(DevError::Again);
        }

        let length = (status & RX_DESC3_LEN_MASK) as usize;
        let valid = status & (DESC3_FD | DESC3_LD) == (DESC3_FD | DESC3_LD)
            && status & RX_DESC3_ERROR == 0
            && (1..=NET_BUFFER_LEN).contains(&length);
        let packet = if valid {
            H::dma_invalidate(
                self.storage.buffers.rx[index].as_ptr() as usize,
                length,
            );
            let mut packet = self.net_pool.alloc_boxed().ok_or(DevError::NoMemory)?;
            packet.set_packet_len(length);
            packet
                .packet_mut()
                .copy_from_slice(&self.storage.buffers.rx[index][..length]);
            Some(packet.into_buf_ptr())
        } else {
            warn!("JH7110 GMAC1 dropped RX descriptor {index}: status={status:#010x}");
            None
        };

        let rx_pa = Self::dma_address(self.storage.buffers.rx[index].as_ptr() as usize)?;
        self.storage.rings.rx[index].arm_rx(rx_pa);
        H::dma_clean(desc_addr, DmaDesc::size());
        H::dma_barrier();
        let desc_pa = Self::dma_address(desc_addr)?;
        self.write_reg(DMA_CH0_RXDESC_TAIL, desc_pa as u32);
        self.rx_index = (index + 1) % RING_SIZE;
        packet.ok_or(DevError::Again)
    }

    fn reclaim_tx(&mut self) {
        while self.tx_in_flight != 0 {
            let index = self.tx_clean;
            let desc_addr = &self.storage.rings.tx[index] as *const DmaDesc as usize;
            H::dma_invalidate(desc_addr, DmaDesc::size());
            if self.storage.rings.tx[index].status() & DESC3_OWN != 0 {
                break;
            }
            self.storage.rings.tx[index].clear();
            H::dma_clean(desc_addr, DmaDesc::size());
            self.tx_clean = (index + 1) % RING_SIZE;
            self.tx_in_flight -= 1;
        }
    }

    fn transmit_one(&mut self, tx_buf: NetBufPtr) -> DevResult {
        let tx_buf = unsafe { NetBuf::from_buf_ptr(tx_buf) };
        let length = tx_buf.packet().len();
        if !(1..=NET_BUFFER_LEN).contains(&length) {
            return Err(DevError::InvalidParam);
        }
        self.reclaim_tx();
        if self.tx_in_flight == RING_SIZE {
            return Err(DevError::Again);
        }
        let index = self.tx_submit;
        self.storage.buffers.tx[index][..length].copy_from_slice(tx_buf.packet());
        H::dma_clean(
            self.storage.buffers.tx[index].as_ptr() as usize,
            length,
        );
        let buffer_pa = Self::dma_address(self.storage.buffers.tx[index].as_ptr() as usize)?;
        self.storage.rings.tx[index].arm_tx(buffer_pa, length);
        let desc_addr = &self.storage.rings.tx[index] as *const DmaDesc as usize;
        H::dma_clean(desc_addr, DmaDesc::size());
        H::dma_barrier();

        self.tx_submit = (index + 1) % RING_SIZE;
        self.tx_in_flight += 1;
        let next_addr = &self.storage.rings.tx[self.tx_submit] as *const DmaDesc as usize;
        self.write_reg(DMA_CH0_TXDESC_TAIL, Self::dma_address(next_addr)? as u32);
        Ok(())
    }

    fn mdio_read(&self, phy: u8, register: u8) -> Option<u16> {
        let deadline = H::current_ms().saturating_add(20);
        while self.read_reg(MAC_MDIO_ADDRESS) & MDIO_BUSY != 0 {
            if H::current_ms() >= deadline {
                return None;
            }
        }
        self.write_reg(
            MAC_MDIO_ADDRESS,
            ((phy as u32) << 21)
                | ((register as u32) << 16)
                | MDIO_CLOCK_250_300
                | MDIO_READ
                | MDIO_BUSY,
        );
        let deadline = H::current_ms().saturating_add(20);
        while self.read_reg(MAC_MDIO_ADDRESS) & MDIO_BUSY != 0 {
            if H::current_ms() >= deadline {
                return None;
            }
        }
        Some(self.read_reg(MAC_MDIO_DATA) as u16)
    }

    fn read_mac_address(&self) -> [u8; 6] {
        let low = self.read_reg(MAC_ADDRESS0_LOW);
        let high = self.read_reg(MAC_ADDRESS0_HIGH);
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
        self.write_reg(
            MAC_ADDRESS0_LOW,
            u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]),
        );
        self.write_reg(
            MAC_ADDRESS0_HIGH,
            u16::from_le_bytes([mac[4], mac[5]]) as u32,
        );
    }

    fn valid_mac(mac: [u8; 6]) -> bool {
        mac != [0; 6] && mac != [0xff; 6] && mac[0] & 1 == 0
    }

    fn dma_address(virtual_address: usize) -> DevResult<usize> {
        let address = H::virt_to_phys(virtual_address);
        if address > u32::MAX as usize {
            return Err(DevError::Unsupported);
        }
        Ok(address)
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { read_volatile(self.base.add(offset).cast::<u32>()) }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        unsafe { write_volatile(self.base.add(offset).cast::<u32>(), value) }
    }
}

impl<H: Jh7110Hal> BaseDriverOps for DwmacJh7110<H> {
    fn device_name(&self) -> &str {
        "jh7110-gmac1"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }

    fn irq_num(&self) -> Option<usize> {
        self.irq
    }
}

impl<H: Jh7110Hal> Drop for DwmacJh7110<H> {
    fn drop(&mut self) {
        self.stop_dma();
        DMA_IRQ_ACTIVE.store(false, Ordering::Release);
        DMA_MMIO_BASE.store(0, Ordering::Release);
        DMA_IRQ_EVENTS.store(0, Ordering::Release);
    }
}

impl<H: Jh7110Hal> NetDriverOps for DwmacJh7110<H> {
    fn mac_address(&self) -> EthernetAddress {
        EthernetAddress(self.mac)
    }

    fn can_transmit(&self) -> bool {
        self.tx_in_flight < RING_SIZE
    }

    fn can_receive(&self) -> bool {
        let desc = &self.storage.rings.rx[self.rx_index];
        H::dma_invalidate(desc as *const DmaDesc as usize, DmaDesc::size());
        desc.status() & DESC3_OWN == 0
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
        self.reclaim_tx();
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

    fn poll_complete(&mut self) -> bool {
        self.reclaim_tx();
        let events = DMA_IRQ_EVENTS.swap(0, Ordering::AcqRel);
        if events & DMA_IRQ_FBE != 0 {
            warn!("JH7110 GMAC1 fatal DMA bus error: status={events:#010x}");
        }
        self.can_receive()
    }
}

/// Acknowledges GMAC1 DMA channel interrupts. Packet work stays deferred in
/// the network poll path; the generic IRQ hook wakes its registered waiter.
pub fn handle_interrupt() {
    if !DMA_IRQ_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let base = DMA_MMIO_BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    let status = unsafe { read_volatile((base as *const u8).add(DMA_CH0_STATUS).cast::<u32>()) }
        & DMA_IRQ_MASK;
    if status == 0 {
        return;
    }
    unsafe {
        write_volatile((base as *mut u8).add(DMA_CH0_STATUS).cast::<u32>(), status);
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    DMA_IRQ_EVENTS.fetch_or(status, Ordering::Release);
}
