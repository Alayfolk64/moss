//! Synopsys DWMAC 5.20 (EQoS) driver for StarFive JH7110 GMAC1.
//!
//! Probe leaves DMA stopped. The first network queue operation initializes
//! the rings, then a Clause 22 state machine keeps MAC speed, duplex and flow
//! control synchronized with the external PHY. JH7110 DMA is non-coherent,
//! so every ownership transfer is accompanied by explicit cache maintenance.

use alloc::sync::Arc;
use core::{
    marker::PhantomData,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use log::{info, warn};

use crate::{
    EthernetAddress, NetBuf, NetBufPool, NetBufPtr, NetDriverOps,
    dwmac_common_state::{
        DmaLifecycle, LinkState, LinkTransition, PhyLinkMode, PhySample, RxDescriptorDisposition,
        TxRingState, clause22_gigabit_capable, clause22_link_mode, link_transition,
        previous_ring_index, rx_descriptor_disposition,
    },
    dwmac_jh7110_state::{
        DMA_IRQ_DEFAULT, DMA_IRQ_ERRORS, DMA_IRQ_RX, DMA_IRQ_TX,
        DMA_STATUS_HANDLED, DMA_STATUS_NIE, DMA_STATUS_RBU, DMA_STATUS_RPS, DMA_STATUS_RX,
        DMA_STATUS_TBU, DMA_STATUS_TX, TxClockInversion, axi_blen_mask, dma_events_for_poll,
        dma_requires_recovery, dma_sysbus_mode, effective_fifo_depth, hardware_fifo_depth,
        mac_address_high, mac_packet_filter, masked_irq_enable, mtl_rx_queue_mode,
        mtl_tx_queue_mode, valid_pbl,
    },
};

mod dma;
mod registers;

use self::{
    dma::{
        DESC3_ERROR, DESC3_FD, DESC3_LD, DESC3_OWN, DMA_BUFFER_LEN, DmaDesc, DmaStorage, RING_SIZE,
        RX_DESC3_LEN_MASK,
    },
    registers::*,
};

const NET_BUFFER_LEN: usize = 1526;
const NET_BUFFER_COUNT: usize = 64;
const PHY_POLL_INTERVAL_MS: u64 = 1_000;
const YT8531_PHY_ID: u32 = 0x4f51_e91b;
const YT8531_EXT_ADDR: u8 = 0x1e;
const YT8531_EXT_DATA: u8 = 0x1f;
const YT8531_RGMII_CONFIG1: u16 = 0xa003;
const YT8531_TX_CLOCK_INVERT: u16 = 1 << 14;

/// Validate and encode the Linux binding's seven-cell `snps,blen` property.
pub fn jh7110_axi_blen_mask(values: &[u32]) -> Option<u8> {
    axi_blen_mask(values)
}

/// Return whether a DT PBL value is accepted by the Linux DWMAC binding.
pub const fn jh7110_valid_pbl(value: u32) -> bool {
    valid_pbl(value)
}

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
    pub dma: Jh7110DmaConfig,
    pub phy: Jh7110PhyConfig,
}

#[derive(Clone, Copy, Debug)]
pub struct Jh7110DmaConfig {
    pub tx_pbl: u8,
    pub rx_pbl: u8,
    pub pbl_x8: bool,
    pub fixed_burst: bool,
    pub force_threshold: bool,
    pub tx_fifo_depth: usize,
    pub rx_fifo_depth: usize,
    pub axi: Jh7110AxiConfig,
}

impl Jh7110DmaConfig {
    pub const fn linux_default() -> Self {
        Self {
            tx_pbl: 16,
            rx_pbl: 16,
            pbl_x8: true,
            fixed_burst: false,
            force_threshold: false,
            tx_fifo_depth: 0,
            rx_fifo_depth: 0,
            axi: Jh7110AxiConfig::linux_default(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Jh7110AxiConfig {
    pub write_outstanding_limit: u8,
    pub read_outstanding_limit: u8,
    pub burst_register_bits: u8,
}

impl Jh7110AxiConfig {
    pub const fn linux_default() -> Self {
        Self {
            write_outstanding_limit: 1,
            read_outstanding_limit: 2,
            burst_register_bits: 0x1e,
        }
    }
}

/// Runtime portions of the YT8531 setup. Delay and drive-strength setup stays
/// owned by firmware; MOSS only switches the negotiated-speed TX clock phase.
#[derive(Clone, Copy, Debug)]
pub struct Jh7110PhyConfig {
    pub adjust_tx_clock: bool,
    pub tx_clock_inverted_10: bool,
    pub tx_clock_inverted_100: bool,
    pub tx_clock_inverted_1000: bool,
}

impl Jh7110PhyConfig {
    /// VisionFive 2 v1.3B upstream DT policy: invert only the 100 Mbps clock.
    pub const fn visionfive2_v13b() -> Self {
        Self {
            adjust_tx_clock: true,
            tx_clock_inverted_10: false,
            tx_clock_inverted_100: true,
            tx_clock_inverted_1000: false,
        }
    }

    const fn inversion(self) -> TxClockInversion {
        TxClockInversion {
            speed_10: self.tx_clock_inverted_10,
            speed_100: self.tx_clock_inverted_100,
            speed_1000: self.tx_clock_inverted_1000,
        }
    }
}

static DMA_MMIO_BASE: AtomicUsize = AtomicUsize::new(0);
static DMA_IRQ_ACTIVE: AtomicBool = AtomicBool::new(false);
static DMA_IRQ_EVENTS: AtomicU32 = AtomicU32::new(0);

pub struct DwmacJh7110<H: Jh7110Hal> {
    base: *mut u8,
    mac: [u8; 6],
    phy_address: u8,
    irq: Option<usize>,
    dma: Jh7110DmaConfig,
    phy: Jh7110PhyConfig,
    storage: DmaStorage,
    rx_index: usize,
    tx_ring: TxRingState,
    lifecycle: DmaLifecycle,
    link_state: LinkState,
    next_phy_poll_ms: u64,
    phy_read_failed: bool,
    net_pool: Arc<NetBufPool>,
    _hal: PhantomData<H>,
}

// SAFETY: no `H` value is stored, the DMA storage has singleton ownership,
// mutable queue operations still require `&mut self`, and shared operations
// perform only volatile MMIO/cache maintenance reads. The hard IRQ handler
// communicates with the owner exclusively through atomics and permanent MMIO.
unsafe impl<H: Jh7110Hal> Send for DwmacJh7110<H> {}
unsafe impl<H: Jh7110Hal> Sync for DwmacJh7110<H> {}

impl<H: Jh7110Hal> DwmacJh7110<H> {
    /// Probe GMAC1 while keeping DMA and MAC packet paths stopped.
    ///
    /// # Safety
    /// `base` must exclusively map a JH7110 DWMAC 5.20 register window.
    pub unsafe fn try_new(base: usize, config: Jh7110Config) -> DevResult<Self> {
        let mut dev = Self {
            base: base as *mut u8,
            mac: [0; 6],
            phy_address: config.phy_address,
            irq: config.irq,
            dma: config.dma,
            phy: config.phy,
            storage: DmaStorage::take()?,
            rx_index: 0,
            tx_ring: TxRingState::new(RING_SIZE),
            lifecycle: DmaLifecycle::new(),
            link_state: LinkState::Down,
            next_phy_poll_ms: 0,
            phy_read_failed: false,
            net_pool: NetBufPool::new(NET_BUFFER_COUNT, NET_BUFFER_LEN)?,
            _hal: PhantomData,
        };

        dev.mac = config.mac_address.unwrap_or_else(|| dev.read_mac_address());
        if !Self::valid_mac(dev.mac) {
            dev.mac = [0x02, 0x4d, 0x4f, 0x53, 0x53, 0x01];
            dev.write_mac_address(dev.mac);
            warn!("JH7110 GMAC1 had no firmware MAC; using 02:4d:4f:53:53:01");
        }

        dev.stop_dma();
        dev.write_reg(DMA_CH0_STATUS, DMA_STATUS_HANDLED);
        DMA_MMIO_BASE.store(base, Ordering::Release);
        let bmsr = dev.valid_mdio_read(PHY_BMSR).unwrap_or(u16::MAX);
        info!(
            "JH7110 GMAC1 probed (DMA stopped): version={:#010x}, hw_feature1={:#010x}, \
             mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, phy={} bmsr={:#06x}, link={}",
            dev.read_reg(MAC_VERSION),
            dev.read_reg(MAC_HW_FEATURE1),
            dev.mac[0],
            dev.mac[1],
            dev.mac[2],
            dev.mac[3],
            dev.mac[4],
            dev.mac[5],
            dev.phy_address,
            bmsr,
            bmsr & PHY_BMSR_LINK != 0,
        );
        Ok(dev)
    }

    /// Record that the platform installed the interrupt handler. Sources stay
    /// masked until the first queue operation starts DMA.
    pub fn register_interrupt_handler(&mut self) {
        if self.irq.is_none() {
            return;
        }
        if self.lifecycle.register_irq() {
            self.enable_hardware_interrupts();
        }
    }

    fn enable_hardware_interrupts(&self) {
        debug_assert!(self.lifecycle.irq_active());
        self.write_reg(DMA_CH0_STATUS, DMA_STATUS_HANDLED);
        DMA_IRQ_EVENTS.store(0, Ordering::Release);
        DMA_IRQ_ACTIVE.store(true, Ordering::Release);
        self.write_reg(DMA_CH0_INTERRUPT_ENABLE, DMA_IRQ_DEFAULT & !DMA_IRQ_TX);
        H::dma_barrier();
    }

    fn ensure_started(&mut self) -> DevResult {
        if self.lifecycle.started() {
            return Ok(());
        }
        self.stop_dma();
        self.reset_dma()?;
        self.rx_index = 0;
        self.tx_ring = TxRingState::new(RING_SIZE);
        self.initialize_descriptors();
        self.initialize_mtl_dma_mac();
        self.start_dma_engines();
        if self.lifecycle.start() {
            self.enable_hardware_interrupts();
        }
        self.refresh_phy_link(true, true);
        info!(
            "JH7110 GMAC1 DMA queues started on first network use; link={:?}",
            self.link_state
        );
        Ok(())
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

    fn initialize_mtl_dma_mac(&self) {
        let feature = self.read_reg(MAC_HW_FEATURE1);
        let tx_fifo = effective_fifo_depth(self.dma.tx_fifo_depth, hardware_fifo_depth(feature, 6));
        let rx_fifo = effective_fifo_depth(self.dma.rx_fifo_depth, hardware_fifo_depth(feature, 0));
        self.write_reg(
            MTL_TXQ0_OPERATION_MODE,
            mtl_tx_queue_mode(tx_fifo, self.dma.force_threshold),
        );
        self.write_reg(MTL_TXQ0_QUANTUM_WEIGHT, 0x10);
        self.write_reg(
            MTL_RXQ0_OPERATION_MODE,
            mtl_rx_queue_mode(rx_fifo, self.dma.force_threshold),
        );

        self.write_reg(MAC_RXQ_CTRL0, (self.read_reg(MAC_RXQ_CTRL0) & !3) | 2);
        self.write_reg(MAC_RXQ_CTRL1, self.read_reg(MAC_RXQ_CTRL1) | (1 << 20));
        self.write_reg(
            MAC_PACKET_FILTER,
            mac_packet_filter(self.read_reg(MAC_PACKET_FILTER)),
        );
        // ACS strips pad/FCS from IEEE 802.3 length frames; CST covers Ethernet
        // II type frames. Without both bits RDES3 reports a length including
        // the four-byte FCS, which must never be handed to the network stack.
        self.write_reg(
            MAC_CONFIGURATION,
            self.read_reg(MAC_CONFIGURATION) | MAC_CONFIG_ACS | MAC_CONFIG_CST,
        );
        self.write_mac_address(self.mac);
        self.disable_mac_link();

        self.write_reg(DMA_MODE, self.read_reg(DMA_MODE) | DMA_MODE_DCHE);
        self.write_reg(
            DMA_SYSBUS_MODE,
            dma_sysbus_mode(
                self.read_reg(DMA_SYSBUS_MODE),
                self.dma.axi.write_outstanding_limit,
                self.dma.axi.read_outstanding_limit,
                self.dma.axi.burst_register_bits,
                self.dma.fixed_burst,
            ),
        );
        self.write_reg(
            DMA_CH0_CONTROL,
            (self.read_reg(DMA_CH0_CONTROL) & !(DMA_CH_CONTROL_DSL_MASK | DMA_CH_CONTROL_PBLX8))
                | (DmaDesc::skip_length() << DMA_CH_CONTROL_DSL_SHIFT)
                | if self.dma.pbl_x8 {
                    DMA_CH_CONTROL_PBLX8
                } else {
                    0
                },
        );
        self.write_reg(
            DMA_CH0_TX_CONTROL,
            (self.read_reg(DMA_CH0_TX_CONTROL) & !DMA_TX_CONTROL_TXPBL_MASK)
                | (u32::from(self.dma.tx_pbl) << 16)
                | DMA_TX_CONTROL_OSP,
        );
        self.write_reg(
            DMA_CH0_RX_CONTROL,
            (self.read_reg(DMA_CH0_RX_CONTROL)
                & !(DMA_RX_CONTROL_RXPBL_MASK | DMA_RX_CONTROL_RBSZ_MASK))
                | (u32::from(self.dma.rx_pbl) << 16)
                | ((DMA_BUFFER_LEN as u32) << 1),
        );
        self.write_reg(DMA_CH0_INTERRUPT_ENABLE, 0);
        self.write_reg(DMA_CH0_STATUS, DMA_STATUS_HANDLED);
        info!(
            "JH7110 GMAC1 DMA config: tx_fifo={} rx_fifo={} txpbl={} rxpbl={} pblx8={} \
             threshold={}",
            tx_fifo,
            rx_fifo,
            self.dma.tx_pbl,
            self.dma.rx_pbl,
            self.dma.pbl_x8,
            self.dma.force_threshold,
        );
    }

    fn initialize_descriptors(&mut self) {
        for index in 0..RING_SIZE {
            let rx_pa = H::virt_to_phys(self.storage.buffers.rx[index].as_ptr() as usize);
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

        let tx_pa = H::virt_to_phys(self.storage.rings.tx.as_ptr() as usize);
        let rx_pa = H::virt_to_phys(self.storage.rings.rx.as_ptr() as usize);
        self.write_reg(DMA_CH0_TXDESC_LIST_HI, (tx_pa >> 32) as u32);
        self.write_reg(DMA_CH0_TXDESC_LIST, tx_pa as u32);
        self.write_reg(DMA_CH0_TXDESC_RING_LEN, RING_SIZE as u32 - 1);
        self.write_reg(DMA_CH0_RXDESC_LIST_HI, (rx_pa >> 32) as u32);
        self.write_reg(DMA_CH0_RXDESC_LIST, rx_pa as u32);
        self.write_reg(DMA_CH0_RXDESC_RING_LEN, RING_SIZE as u32 - 1);
        self.write_reg(DMA_CH0_TXDESC_TAIL, tx_pa as u32);
        // EQoS tail pointers are ownership boundaries. An initial TX tail at
        // descriptor 0 means no work; an RX tail at the final descriptor makes
        // the complete pre-owned RX ring visible to DMA. Subsequent updates
        // point at the descriptor just returned (RX) or one past it (TX).
        self.write_reg(
            DMA_CH0_RXDESC_TAIL,
            (rx_pa + (RING_SIZE - 1) * DmaDesc::size()) as u32,
        );
    }

    fn start_dma_engines(&self) {
        self.write_reg(
            DMA_CH0_TX_CONTROL,
            self.read_reg(DMA_CH0_TX_CONTROL) | DMA_TX_CONTROL_ST,
        );
        self.write_reg(
            DMA_CH0_RX_CONTROL,
            self.read_reg(DMA_CH0_RX_CONTROL) | DMA_RX_CONTROL_SR,
        );
        H::dma_barrier();
    }

    fn disable_mac_link(&self) {
        self.write_reg(
            MAC_CONFIGURATION,
            self.read_reg(MAC_CONFIGURATION) & !(MAC_CONFIG_TE | MAC_CONFIG_RE),
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
        self.disable_mac_link();
        H::dma_barrier();
    }

    fn apply_link_mode(&self, mode: PhyLinkMode) -> bool {
        self.disable_mac_link();
        if !self.configure_phy_tx_clock(mode.speed_mbps) {
            return false;
        }
        let speed = match mode.speed_mbps {
            1_000 => 0,
            100 => MAC_CONFIG_PS | MAC_CONFIG_FES,
            10 => MAC_CONFIG_PS,
            _ => return false,
        };
        let duplex = if mode.full_duplex { MAC_CONFIG_DM } else { 0 };
        let config = (self.read_reg(MAC_CONFIGURATION)
            & !(MAC_CONFIG_PS | MAC_CONFIG_FES | MAC_CONFIG_DM))
            | speed
            | duplex;
        self.write_reg(MAC_CONFIGURATION, config);

        let tx_flow = (0xffff << 16)
            | if mode.full_duplex && mode.pause_tx {
                MAC_TX_FLOW_ENABLE
            } else {
                0
            };
        self.write_reg(MAC_Q0_TX_FLOW_CTRL, tx_flow);
        self.write_reg(
            MAC_RX_FLOW_CTRL,
            if mode.full_duplex && mode.pause_rx {
                MAC_RX_FLOW_ENABLE
            } else {
                0
            },
        );
        H::dma_barrier();
        self.write_reg(
            MAC_CONFIGURATION,
            self.read_reg(MAC_CONFIGURATION) | MAC_CONFIG_TE | MAC_CONFIG_RE,
        );
        H::dma_barrier();
        true
    }

    fn receive_one(&mut self) -> DevResult<NetBufPtr> {
        self.ensure_started()?;
        if !self.link_state.is_up() {
            return Err(DevError::Again);
        }
        for _ in 0..RING_SIZE {
            let index = self.rx_index;
            let desc_addr = &self.storage.rings.rx[index] as *const DmaDesc as usize;
            H::dma_invalidate(desc_addr, DmaDesc::size());
            let status = self.storage.rings.rx[index].status();
            if status & DESC3_OWN != 0 {
                return Err(DevError::Again);
            }
            self.rx_index = (index + 1) % RING_SIZE;

            let length = (status & RX_DESC3_LEN_MASK) as usize;
            let packet_len = (status & (DESC3_FD | DESC3_LD | DESC3_ERROR)
                == (DESC3_FD | DESC3_LD)
                && (1..=NET_BUFFER_LEN).contains(&length))
            .then_some(length);
            let mut allocated = if packet_len.is_some() {
                self.net_pool.alloc_boxed()
            } else {
                None
            };
            let disposition = rx_descriptor_disposition(packet_len, allocated.is_some());
            let result = match disposition {
                RxDescriptorDisposition::Deliver(packet_len) => {
                    H::dma_invalidate(self.storage.buffers.rx[index].as_ptr() as usize, packet_len);
                    let mut packet = allocated.take().expect("RX allocation checked above");
                    packet.set_packet_len(packet_len);
                    packet
                        .packet_mut()
                        .copy_from_slice(&self.storage.buffers.rx[index][..packet_len]);
                    Ok(Some(packet.into_buf_ptr()))
                }
                RxDescriptorDisposition::DropInvalid => {
                    warn!("JH7110 GMAC1 dropped RX descriptor {index}: status={status:#010x}");
                    Ok(None)
                }
                RxDescriptorDisposition::DropNoMemory => Err(DevError::NoMemory),
            };

            debug_assert!(disposition.must_rearm());
            let rx_pa = H::virt_to_phys(self.storage.buffers.rx[index].as_ptr() as usize);
            self.storage.rings.rx[index].arm_rx(rx_pa);
            H::dma_clean(desc_addr, DmaDesc::size());
            H::dma_barrier();
            self.write_reg(DMA_CH0_RXDESC_TAIL, H::virt_to_phys(desc_addr) as u32);
            match result {
                Ok(Some(packet)) => return Ok(packet),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
        Err(DevError::Again)
    }

    fn reclaim_tx(&mut self) {
        while self.tx_ring.in_flight() != 0 {
            let index = self.tx_ring.clean_index();
            let desc_addr = &self.storage.rings.tx[index] as *const DmaDesc as usize;
            H::dma_invalidate(desc_addr, DmaDesc::size());
            let status = self.storage.rings.tx[index].status();
            if status & DESC3_OWN != 0 {
                break;
            }
            if status & DESC3_ERROR != 0 {
                warn!("JH7110 GMAC1 TX descriptor {index} completed with error: {status:#010x}");
            }
            self.storage.rings.tx[index].clear();
            H::dma_clean(desc_addr, DmaDesc::size());
            let completed = self.tx_ring.complete_one();
            debug_assert!(completed);
        }
    }

    fn transmit_one(&mut self, tx_buf: NetBufPtr) -> DevResult {
        // NetDriverOps transfers ownership even when submission fails. Rebuild
        // the box before any fallible check so every return path releases it.
        let tx_buf = unsafe { NetBuf::from_buf_ptr(tx_buf) };
        self.ensure_started()?;
        if !self.link_state.is_up() {
            return Err(DevError::Again);
        }
        let length = tx_buf.packet().len();
        if !(1..=NET_BUFFER_LEN).contains(&length) {
            return Err(DevError::InvalidParam);
        }
        self.reclaim_tx();
        let index = self.tx_ring.submit_index().ok_or(DevError::Again)?;
        let desc_addr = &self.storage.rings.tx[index] as *const DmaDesc as usize;
        H::dma_invalidate(desc_addr, DmaDesc::size());
        if self.storage.rings.tx[index].status() & DESC3_OWN != 0 {
            return Err(DevError::Again);
        }

        self.storage.buffers.tx[index][..length].copy_from_slice(tx_buf.packet());
        H::dma_clean(self.storage.buffers.tx[index].as_ptr() as usize, length);
        let buffer_pa = H::virt_to_phys(self.storage.buffers.tx[index].as_ptr() as usize);
        self.storage.rings.tx[index].arm_tx(buffer_pa, length);
        H::dma_clean(desc_addr, DmaDesc::size());
        H::dma_barrier();
        let reserved = self.tx_ring.reserve();
        debug_assert_eq!(reserved, Some(index));

        if self.lifecycle.irq_active() {
            self.write_reg(
                DMA_CH0_INTERRUPT_ENABLE,
                self.read_reg(DMA_CH0_INTERRUPT_ENABLE) | DMA_IRQ_TX | DMA_STATUS_NIE,
            );
        }
        let next = (index + 1) % RING_SIZE;
        let next_addr = &self.storage.rings.tx[next] as *const DmaDesc as usize;
        self.write_reg(DMA_CH0_TXDESC_TAIL, H::virt_to_phys(next_addr) as u32);
        Ok(())
    }

    fn finish_interrupt_poll(&mut self) -> bool {
        if self.ensure_started().is_err() {
            return false;
        }
        let link_changed = self.refresh_phy_link(false, false);
        self.reclaim_tx();
        let irq_active = self.lifecycle.irq_active();
        let queued_events = DMA_IRQ_EVENTS.swap(0, Ordering::AcqRel);
        let hardware_status = if irq_active {
            0
        } else {
            let status = self.read_reg(DMA_CH0_STATUS) & DMA_STATUS_HANDLED;
            if status != 0 {
                self.write_reg(DMA_CH0_STATUS, status);
            }
            status
        };
        let events = dma_events_for_poll(irq_active, queued_events, hardware_status);

        if dma_requires_recovery(events, self.tx_ring.in_flight()) {
            warn!(
                "JH7110 GMAC1 unrecoverable DMA state {events:#010x} with {} TX descriptors; \
                 resetting DMA queues",
                self.tx_ring.in_flight()
            );
            return self.recover_dma();
        }
        if events & (DMA_STATUS_RBU | DMA_STATUS_RPS) != 0 {
            // RX tail is inclusive: publish the descriptor most recently
            // returned to DMA, which is one slot behind the next consumer.
            let tail = previous_ring_index(self.rx_index, RING_SIZE);
            let desc = &self.storage.rings.rx[tail] as *const DmaDesc as usize;
            self.write_reg(DMA_CH0_RXDESC_TAIL, H::virt_to_phys(desc) as u32);
        }
        if events & DMA_STATUS_TBU != 0 && self.tx_ring.in_flight() != 0 {
            // TX tail is exclusive: re-publish the producer boundary. Using
            // the clean index here can exclude all descriptors still owned by DMA.
            let desc = &self.storage.rings.tx[self.tx_ring.tail_index()] as *const DmaDesc as usize;
            self.write_reg(DMA_CH0_TXDESC_TAIL, H::virt_to_phys(desc) as u32);
        }
        if !irq_active {
            return link_changed;
        }

        let rx_ready = self.rx_descriptor_ready();
        let tx_ready = self.tx_descriptor_complete();
        if rx_ready || tx_ready {
            DMA_IRQ_EVENTS.fetch_or(
                (if rx_ready { DMA_STATUS_RX } else { 0 })
                    | if tx_ready { DMA_STATUS_TX } else { 0 },
                Ordering::Release,
            );
            return true;
        }

        let mut enable = self.read_reg(DMA_CH0_INTERRUPT_ENABLE) | DMA_IRQ_RX | DMA_IRQ_ERRORS;
        if self.tx_ring.in_flight() != 0 {
            enable |= DMA_IRQ_TX;
        } else {
            enable &= !DMA_IRQ_TX;
        }
        enable |= DMA_STATUS_NIE;
        self.write_reg(DMA_CH0_INTERRUPT_ENABLE, enable);
        H::dma_barrier();

        let status = self.read_reg(DMA_CH0_STATUS) & DMA_STATUS_HANDLED;
        if status != 0 {
            self.write_reg(DMA_CH0_INTERRUPT_ENABLE, masked_irq_enable(enable, status));
            self.write_reg(DMA_CH0_STATUS, status);
            DMA_IRQ_EVENTS.fetch_or(status, Ordering::Release);
            return true;
        }
        link_changed
    }

    fn recover_dma(&mut self) -> bool {
        self.stop_dma();
        self.lifecycle.stop();
        self.link_state = LinkState::Down;
        DMA_IRQ_ACTIVE.store(false, Ordering::Release);
        if self.reset_dma().is_ok() {
            self.rx_index = 0;
            self.tx_ring = TxRingState::new(RING_SIZE);
            self.initialize_descriptors();
            self.initialize_mtl_dma_mac();
            self.start_dma_engines();
            if self.lifecycle.start() {
                self.enable_hardware_interrupts();
            }
            self.refresh_phy_link(true, true);
            return false;
        }
        warn!("JH7110 GMAC1 DMA recovery failed; device remains stopped");
        false
    }

    fn rx_descriptor_ready(&self) -> bool {
        let desc = &self.storage.rings.rx[self.rx_index];
        H::dma_invalidate(desc as *const DmaDesc as usize, DmaDesc::size());
        desc.status() & DESC3_OWN == 0
    }

    fn tx_descriptor_complete(&self) -> bool {
        if self.tx_ring.in_flight() == 0 {
            return false;
        }
        let desc = &self.storage.rings.tx[self.tx_ring.clean_index()];
        H::dma_invalidate(desc as *const DmaDesc as usize, DmaDesc::size());
        desc.status() & DESC3_OWN == 0
    }

    fn clause22_phy_sample(&self) -> PhySample {
        // BMSR link state is latch-low, so it must be read twice.
        let Some(_) = self.valid_mdio_read(PHY_BMSR) else {
            return PhySample::Unavailable;
        };
        let Some(bmsr) = self.valid_mdio_read(PHY_BMSR) else {
            return PhySample::Unavailable;
        };
        if bmsr == 0 || bmsr == u16::MAX {
            return PhySample::Unavailable;
        }
        if bmsr & PHY_BMSR_LINK == 0 {
            return PhySample::Down;
        }
        let Some(bmcr) = self.valid_mdio_read(PHY_BMCR) else {
            return PhySample::Unavailable;
        };
        if bmcr & PHY_BMCR_AUTONEG_ENABLE != 0 && bmsr & PHY_BMSR_AUTONEG_COMPLETE == 0 {
            return PhySample::Down;
        }

        let registers = if bmcr & PHY_BMCR_AUTONEG_ENABLE != 0 {
            let Some(advertise) = self.valid_mdio_read(PHY_ADVERTISE) else {
                return PhySample::Unavailable;
            };
            let Some(link_partner) = self.valid_mdio_read(PHY_LINK_PARTNER) else {
                return PhySample::Unavailable;
            };
            let gigabit = if bmsr & PHY_BMSR_EXTENDED_STATUS != 0 {
                let Some(extended_status) = self.valid_mdio_read(PHY_EXTENDED_STATUS) else {
                    return PhySample::Unavailable;
                };
                clause22_gigabit_capable(bmsr, extended_status)
            } else {
                false
            };
            let (control_1000, status_1000) = if gigabit {
                let Some(control) = self.valid_mdio_read(PHY_CONTROL_1000) else {
                    return PhySample::Unavailable;
                };
                let Some(status) = self.valid_mdio_read(PHY_STATUS_1000) else {
                    return PhySample::Unavailable;
                };
                (control, status)
            } else {
                (0, 0)
            };
            (advertise, link_partner, control_1000, status_1000)
        } else {
            (0, 0, 0, 0)
        };
        clause22_link_mode(
            bmcr,
            bmsr,
            registers.0,
            registers.1,
            registers.2,
            registers.3,
        )
        .map_or(PhySample::Down, PhySample::Up)
    }

    fn refresh_phy_link(&mut self, force: bool, reapply_current: bool) -> bool {
        let now = H::current_ms();
        if !force && now < self.next_phy_poll_ms {
            return false;
        }
        self.next_phy_poll_ms = now.saturating_add(PHY_POLL_INTERVAL_MS);
        let sample = self.clause22_phy_sample();
        if sample == PhySample::Unavailable {
            if !self.phy_read_failed {
                warn!("JH7110 GMAC1 PHY became unavailable; preserving prior link state");
            }
            self.phy_read_failed = true;
            if reapply_current {
                match self.link_state {
                    LinkState::Down => self.disable_mac_link(),
                    LinkState::Up(mode) => {
                        if !self.apply_link_mode(mode) {
                            self.link_state = LinkState::Down;
                        }
                    }
                }
            }
            return false;
        }
        if self.phy_read_failed {
            info!("JH7110 GMAC1 PHY MDIO access recovered");
            self.phy_read_failed = false;
        }

        match link_transition(self.link_state, sample) {
            LinkTransition::None => {
                if reapply_current {
                    match self.link_state {
                        LinkState::Down => self.disable_mac_link(),
                        LinkState::Up(mode) => {
                            if !self.apply_link_mode(mode) {
                                self.link_state = LinkState::Down;
                            }
                        }
                    }
                }
                false
            }
            LinkTransition::Down => {
                self.disable_mac_link();
                self.link_state = LinkState::Down;
                info!("JH7110 GMAC1 link down");
                true
            }
            LinkTransition::Up(mode) | LinkTransition::Reconfigure(mode) => {
                if !self.apply_link_mode(mode) {
                    self.link_state = LinkState::Down;
                    warn!("JH7110 GMAC1 link setup deferred after PHY phase update failure");
                    return false;
                }
                self.link_state = LinkState::Up(mode);
                info!(
                    "JH7110 GMAC1 link up: {} Mbps, {}, pause_rx={}, pause_tx={}",
                    mode.speed_mbps,
                    if mode.full_duplex { "full" } else { "half" },
                    mode.pause_rx,
                    mode.pause_tx,
                );
                true
            }
        }
    }

    fn configure_phy_tx_clock(&self, speed_mbps: u16) -> bool {
        if !self.phy.adjust_tx_clock {
            return true;
        }
        let (Some(id1), Some(id2)) = (self.valid_mdio_read(PHY_ID1), self.valid_mdio_read(PHY_ID2))
        else {
            return false;
        };
        let phy_id = (u32::from(id1) << 16) | u32::from(id2);
        if phy_id != YT8531_PHY_ID {
            warn!("JH7110 GMAC1 PHY {phy_id:#010x} is not YT8531; skipping vendor phase write");
            return true;
        }
        let Some(inverted) = self.phy.inversion().for_speed(speed_mbps) else {
            return false;
        };
        let Some(current) = self.mdio_extended_read(YT8531_RGMII_CONFIG1) else {
            return false;
        };
        let configured = if inverted {
            current | YT8531_TX_CLOCK_INVERT
        } else {
            current & !YT8531_TX_CLOCK_INVERT
        };
        current == configured || self.mdio_extended_write(YT8531_RGMII_CONFIG1, configured)
    }

    fn mdio_extended_read(&self, register: u16) -> Option<u16> {
        self.mdio_write(self.phy_address, YT8531_EXT_ADDR, register)?;
        self.valid_mdio_read(YT8531_EXT_DATA)
    }

    fn mdio_extended_write(&self, register: u16, value: u16) -> bool {
        self.mdio_write(self.phy_address, YT8531_EXT_ADDR, register)
            .is_some()
            && self
                .mdio_write(self.phy_address, YT8531_EXT_DATA, value)
                .is_some()
    }

    fn valid_mdio_read(&self, register: u8) -> Option<u16> {
        self.mdio_read(self.phy_address, register)
            .filter(|value| *value != u16::MAX)
    }

    fn wait_mdio_idle(&self) -> bool {
        let deadline = H::current_ms().saturating_add(20);
        while self.read_reg(MAC_MDIO_ADDRESS) & MDIO_BUSY != 0 {
            if H::current_ms() >= deadline {
                return false;
            }
            core::hint::spin_loop();
        }
        true
    }

    fn mdio_read(&self, phy: u8, register: u8) -> Option<u16> {
        if !self.wait_mdio_idle() {
            return None;
        }
        self.write_reg(
            MAC_MDIO_ADDRESS,
            ((phy as u32) << 21)
                | ((register as u32) << 16)
                | MDIO_CLOCK_250_300
                | MDIO_READ
                | MDIO_BUSY,
        );
        self.wait_mdio_idle()
            .then(|| self.read_reg(MAC_MDIO_DATA) as u16)
    }

    fn mdio_write(&self, phy: u8, register: u8, value: u16) -> Option<()> {
        if !self.wait_mdio_idle() {
            return None;
        }
        self.write_reg(MAC_MDIO_DATA, u32::from(value));
        self.write_reg(
            MAC_MDIO_ADDRESS,
            ((phy as u32) << 21)
                | ((register as u32) << 16)
                | MDIO_CLOCK_250_300
                | MDIO_WRITE
                | MDIO_BUSY,
        );
        self.wait_mdio_idle().then_some(())
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
        // DWMAC4/5 programs the high word before the low word.  The low-word
        // write commits the complete perfect-match address on implementations
        // such as JH7110 EQoS; reversing the order leaves broadcasts working
        // but causes unicast frames for address 0 to be rejected.
        self.write_reg(MAC_ADDRESS0_HIGH, mac_address_high(mac));
        self.write_reg(
            MAC_ADDRESS0_LOW,
            u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]),
        );
    }

    fn valid_mac(mac: [u8; 6]) -> bool {
        mac != [0; 6] && mac != [0xff; 6] && mac[0] & 1 == 0
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
        if self.lifecycle.irq_registered() {
            self.irq
        } else {
            None
        }
    }
}

impl<H: Jh7110Hal> Drop for DwmacJh7110<H> {
    fn drop(&mut self) {
        self.stop_dma();
        self.lifecycle.stop();
        DMA_IRQ_ACTIVE.store(false, Ordering::Release);
        DMA_MMIO_BASE.store(0, Ordering::Release);
        DMA_IRQ_EVENTS.store(0, Ordering::Release);
    }
}

impl<H: Jh7110Hal> NetDriverOps for DwmacJh7110<H> {
    fn irq_handler_registered(&self) -> bool {
        self.lifecycle.irq_registered()
    }

    fn mac_address(&self) -> EthernetAddress {
        EthernetAddress(self.mac)
    }

    fn can_transmit(&self) -> bool {
        if !self.lifecycle.started() || !self.link_state.is_up() {
            return false;
        }
        self.tx_ring.submit_index().is_some_and(|index| {
            let desc = &self.storage.rings.tx[index];
            H::dma_invalidate(desc as *const DmaDesc as usize, DmaDesc::size());
            desc.status() & DESC3_OWN == 0
        })
    }

    fn can_receive(&self) -> bool {
        self.lifecycle.started() && self.link_state.is_up() && self.rx_descriptor_ready()
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
        self.ensure_started()?;
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
        self.finish_interrupt_poll()
    }
}

/// The hard handler masks asserted sources, acknowledges them, and queues
/// deferred descriptor work for `NetDriverOps::poll_complete`.
pub fn handle_interrupt() {
    if !DMA_IRQ_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let base = DMA_MMIO_BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    let read =
        |offset: usize| unsafe { read_volatile((base as *const u8).add(offset).cast::<u32>()) };
    let write = |offset: usize, value: u32| unsafe {
        write_volatile((base as *mut u8).add(offset).cast::<u32>(), value)
    };
    let status = read(DMA_CH0_STATUS) & DMA_STATUS_HANDLED;
    if status == 0 {
        return;
    }
    write(
        DMA_CH0_INTERRUPT_ENABLE,
        masked_irq_enable(read(DMA_CH0_INTERRUPT_ENABLE), status),
    );
    write(DMA_CH0_STATUS, status);
    core::sync::atomic::fence(Ordering::SeqCst);
    DMA_IRQ_EVENTS.fetch_or(status, Ordering::Release);
}
