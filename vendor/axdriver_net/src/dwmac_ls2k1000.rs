//! Synopsys DWMAC 3.70a driver for Loongson 2K1000LA.
//!
//! The register programming, enhanced-descriptor layout, and device lifecycle
//! follow Linux's dwmac1000/stmmac driver.  MOSS has no netdev `ndo_open`
//! callback, so the first queue operation performs Linux's open-time DMA setup.
//! Probe only identifies and registers the DT-described device; it leaves DMA
//! and device interrupt sources stopped.

use alloc::sync::Arc;
use core::{
    marker::PhantomData,
    mem::size_of,
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
        rx_descriptor_disposition,
    },
    dwmac_ls2k1000_state::*,
};

// Keep the upstream queue depth of 128. The buffers live in a static DMA
// section so the resulting 512 KiB allocation never lands on a kernel stack.
const RING_SIZE: usize = 128;
const DMA_BUFFER_LEN: usize = 2048;
const NET_BUFFER_LEN: usize = 1526;
const NET_BUFFER_COUNT: usize = 64;
const PHY_POLL_INTERVAL_MS: u64 = 1_000;

const GMAC_CONTROL: usize = 0x0000;
const GMAC_FRAME_FILTER: usize = 0x0004;
const GMAC_MII_ADDR: usize = 0x0010;
const GMAC_MII_DATA: usize = 0x0014;
const GMAC_FLOW_CONTROL: usize = 0x0018;
const GMAC_VERSION: usize = 0x0020;
const GMAC_PMT: usize = 0x002c;
const GMAC_LPI_CONTROL_STATUS: usize = 0x0030;
const GMAC_INT_STATUS: usize = 0x0038;
const GMAC_INT_MASK: usize = 0x003c;
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
const DMA_HW_FEATURE: usize = 0x1058;

const DMA_BUS_MODE_RESET: u32 = 1;
const DMA_HW_FEATURE_ENHANCED_DESCRIPTOR: u32 = 1 << 24;
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
const GMAC_FLOW_CONTROL_PAUSE_MASK: u32 = 0xffff_0000;
const GMAC_FLOW_CONTROL_UNICAST_PAUSE: u32 = 1 << 3;
const GMAC_FLOW_CONTROL_RX_ENABLE: u32 = 1 << 2;
const GMAC_FLOW_CONTROL_TX_ENABLE: u32 = 1 << 1;
const GMAC_INT_STATUS_RGMII: u32 = 1 << 0;
const GMAC_INT_STATUS_PMT: u32 = 1 << 3;
const GMAC_INT_STATUS_LPI: u32 = 1 << 10;

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
const TX_DESC_ERROR: u32 = 1 << 15;

const MII_BUSY: u32 = 1;
const MII_CLOCK_RANGE_4: u32 = 1 << 4;
const MII_PHY_SHIFT: u32 = 11;
const MII_REG_SHIFT: u32 = 6;
const PHY_BMCR: u8 = 0;
const PHY_BMSR: u8 = 1;
const PHY_ADVERTISE: u8 = 4;
const PHY_LINK_PARTNER: u8 = 5;
const PHY_CONTROL_1000: u8 = 9;
const PHY_STATUS_1000: u8 = 10;
const PHY_EXTENDED_STATUS: u8 = 15;
const PHY_BMSR_LINK: u16 = 1 << 2;
const PHY_BMSR_AUTONEG_COMPLETE: u16 = 1 << 5;
const PHY_BMSR_EXTENDED_STATUS: u16 = 1 << 8;
const PHY_BMCR_AUTONEG_ENABLE: u16 = 1 << 12;

/// Validate and encode the Linux binding's seven-cell `snps,blen` property.
pub fn dwmac_axi_blen_mask(values: &[u32]) -> Option<u8> {
    axi_blen_mask(values)
}

/// Return whether a DT PBL value is accepted by the Linux DWMAC binding.
pub const fn dwmac_valid_pbl(value: u32) -> bool {
    valid_pbl(value)
}

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
    pub dma: DwmacDmaConfig,
}

/// Linux-compatible `snps,*` DMA properties from the DWMAC DT node.
#[derive(Clone, Copy, Debug)]
pub struct DwmacDmaConfig {
    pub pbl: u8,
    pub tx_pbl: u8,
    pub rx_pbl: u8,
    pub pbl_x8: bool,
    pub fixed_burst: bool,
    pub mixed_burst: bool,
    pub address_aligned: bool,
    pub axi: Option<DwmacAxiConfig>,
}

impl DwmacDmaConfig {
    /// Defaults used by Linux's `stmmac_probe_config_dt`.
    pub const fn linux_default() -> Self {
        Self {
            pbl: 8,
            tx_pbl: 0,
            rx_pbl: 0,
            pbl_x8: true,
            fixed_burst: false,
            mixed_burst: false,
            address_aligned: false,
            axi: None,
        }
    }
}

/// Parsed `snps,axi-config` child node. `burst_register_bits` uses the DWMAC
/// AXI BLEN register representation, not raw byte lengths.
#[derive(Clone, Copy, Debug)]
pub struct DwmacAxiConfig {
    pub write_outstanding_limit: u8,
    pub read_outstanding_limit: u8,
    pub burst_register_bits: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DmaDesc {
    status: u32,
    length: u32,
    buffer1: u32,
    buffer2: u32,
    extended_status: u32,
    reserved: u32,
    timestamp_low: u32,
    timestamp_high: u32,
}

const EMPTY_DESC: DmaDesc = DmaDesc {
    status: 0,
    length: 0,
    buffer1: 0,
    buffer2: 0,
    extended_status: 0,
    reserved: 0,
    timestamp_low: 0,
    timestamp_high: 0,
};

// DWMAC 3.70a selects the enhanced descriptor bit layout reported by its HW
// feature register.  In ring mode Linux pairs that layout with CSR0.ATDS and
// an eight-word descriptor stride.  A four-word stride with enhanced RX/TX
// end-of-ring bits lets DMA walk past the ring into unrelated memory.
const _: () = assert!(size_of::<DmaDesc>() == 32);

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
static DMA_MMIO_BASE: AtomicUsize = AtomicUsize::new(0);
static DMA_IRQ_EVENTS: AtomicU32 = AtomicU32::new(0);
static DMA_IRQ_ACTIVE: AtomicBool = AtomicBool::new(false);

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
    dma: DwmacDmaConfig,
    rings: &'static mut DescriptorRings,
    buffers: &'static mut DmaBuffers,
    rx_index: usize,
    tx_ring: TxRingState,
    lifecycle: DmaLifecycle,
    link_state: LinkState,
    next_phy_poll_ms: u64,
    phy_read_failed: bool,
    phy_unavailable_reported: bool,
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
            dma: config.dma,
            rings: unsafe { &mut *core::ptr::addr_of_mut!(DMA_RINGS) },
            buffers: unsafe { &mut *core::ptr::addr_of_mut!(DMA_BUFFERS) },
            rx_index: 0,
            tx_ring: TxRingState::new(RING_SIZE),
            lifecycle: DmaLifecycle::new(),
            link_state: LinkState::Down,
            next_phy_poll_ms: 0,
            phy_read_failed: false,
            phy_unavailable_reported: false,
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
        dev.write_reg(GMAC_INT_MASK, gmac_interrupt_mask(false));
        dev.write_reg(DMA_INTERRUPT_ENABLE, 0);
        dev.write_reg(DMA_STATUS, u32::MAX);
        DMA_MMIO_BASE.store(base, Ordering::Release);

        let phy_status = dev.mdio_read(dev.phy_address, PHY_BMSR).unwrap_or(0xffff);
        let rgmii = dev.read_reg(GMAC_RGSMII_STATUS);
        info!(
            "LS2K1000 GMAC0 probed (DMA stopped): version={:#010x}, \
             mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, phy0_bmsr={:#06x}, rgmii={:#010x}, \
             link={}",
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

    /// Records that the platform installed the DT-provided interrupt handler.
    /// Hardware sources stay masked until the first queue operation starts DMA.
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
        self.write_reg(DMA_STATUS, DMA_STATUS_HANDLED);
        DMA_IRQ_EVENTS.store(0, Ordering::Release);
        DMA_IRQ_ACTIVE.store(true, Ordering::Release);
        self.write_reg(DMA_INTERRUPT_ENABLE, DMA_IRQ_DEFAULT);
        H::dma_barrier();
    }

    fn ensure_started(&mut self) -> DevResult {
        if self.lifecycle.started() {
            return Ok(());
        }

        // Linux stmmac performs these operations from stmmac_open via
        // stmmac_hw_setup, not from platform probe.
        self.stop_tx_rx();
        self.reset_dma()?;
        self.rx_index = 0;
        self.tx_ring = TxRingState::new(RING_SIZE);
        self.initialize_descriptors()?;
        self.initialize_mac()?;
        if self.lifecycle.start() {
            self.enable_hardware_interrupts();
        }
        self.refresh_phy_link(true, true);
        info!(
            "LS2K1000 GMAC0 DMA queues started on first network use; link={:?}",
            self.link_state
        );
        Ok(())
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
                ..EMPTY_DESC
            };
            self.rings.tx[index] = DmaDesc {
                status: if ring_end { TX_DESC_END_RING } else { 0 },
                length: 0,
                buffer1: tx_buffer_pa,
                ..EMPTY_DESC
            };
        }

        H::dma_barrier();
        let rx_desc_pa = Self::dma_address(self.rings.rx.as_ptr() as usize)?;
        let tx_desc_pa = Self::dma_address(self.rings.tx.as_ptr() as usize)?;
        self.write_reg(DMA_RX_DESC_BASE, rx_desc_pa);
        self.write_reg(DMA_TX_DESC_BASE, tx_desc_pa);
        info!(
            "LS2K1000 GMAC0 enhanced DMA rings: descriptor_size={} tx={:#x} rx={:#x}",
            size_of::<DmaDesc>(),
            tx_desc_pa,
            rx_desc_pa,
        );
        Ok(())
    }

    fn initialize_mac(&mut self) -> DevResult {
        let hardware_features = self.read_reg(DMA_HW_FEATURE);
        if hardware_features & DMA_HW_FEATURE_ENHANCED_DESCRIPTOR == 0 {
            warn!(
                "LS2K1000 GMAC0 lacks enhanced descriptor support: \
                 DMA_HW_FEATURE={hardware_features:#010x}"
            );
            return Err(DevError::BadState);
        }
        self.write_mac_address(self.mac);
        self.write_reg(GMAC_INT_MASK, gmac_interrupt_mask(false));
        self.write_reg(DMA_INTERRUPT_ENABLE, 0);
        self.write_reg(DMA_STATUS, u32::MAX);
        self.write_reg(
            DMA_BUS_MODE,
            linux_dma_bus_mode_config(
                self.read_reg(DMA_BUS_MODE),
                DmaBusConfig {
                    tx_pbl: u32::from(if self.dma.tx_pbl == 0 {
                        self.dma.pbl
                    } else {
                        self.dma.tx_pbl
                    }),
                    rx_pbl: u32::from(if self.dma.rx_pbl == 0 {
                        self.dma.pbl
                    } else {
                        self.dma.rx_pbl
                    }),
                    pbl_x8: self.dma.pbl_x8,
                    fixed_burst: self.dma.fixed_burst,
                    mixed_burst: self.dma.mixed_burst,
                    address_aligned: self.dma.address_aligned,
                    extended_desc: true,
                },
            ),
        );
        info!(
            "LS2K1000 GMAC0 DMA descriptor mode: HW_FEATURE={hardware_features:#010x} \
             BUS_MODE={:#010x} ATDS=true",
            self.read_reg(DMA_BUS_MODE),
        );
        if let Some(axi) = self.dma.axi {
            self.write_reg(
                DMA_AXI_BUS_MODE,
                linux_axi_bus_mode_config(
                    self.read_reg(DMA_AXI_BUS_MODE),
                    u32::from(axi.write_outstanding_limit),
                    u32::from(axi.read_outstanding_limit),
                    u32::from(axi.burst_register_bits),
                ),
            );
        }

        self.write_reg(
            GMAC_CONTROL,
            linux_mac_core_config(self.read_reg(GMAC_CONTROL)),
        );
        self.write_reg(GMAC_FRAME_FILTER, linux_mac_frame_filter());

        let dma_control = (self.read_reg(DMA_CONTROL) & !DMA_CONTROL_FLOW_BITS)
            | DMA_CONTROL_STORE_AND_FORWARD
            | DMA_CONTROL_OPERATE_SECOND_FRAME;
        self.write_reg(DMA_CONTROL, dma_control);
        self.write_reg(GMAC_FLOW_CONTROL, GMAC_FLOW_CONTROL_UNICAST_PAUSE);
        self.disable_mac_link();
        self.write_reg(
            DMA_CONTROL,
            self.read_reg(DMA_CONTROL) | DMA_CONTROL_START_TX | DMA_CONTROL_START_RX,
        );
        H::dma_barrier();
        self.write_reg(DMA_RX_POLL_DEMAND, 0);
        Ok(())
    }

    fn disable_mac_link(&self) {
        self.write_reg(
            GMAC_CONTROL,
            self.read_reg(GMAC_CONTROL) & !(GMAC_CONTROL_TX_ENABLE | GMAC_CONTROL_RX_ENABLE),
        );
        H::dma_barrier();
    }

    fn apply_link_mode(&self, mode: PhyLinkMode) {
        self.disable_mac_link();
        let speed = match mode.speed_mbps {
            1_000 => 0,
            100 => GMAC_CONTROL_MII_PORT | GMAC_CONTROL_FAST_ETHERNET_SPEED,
            10 => GMAC_CONTROL_MII_PORT,
            _ => return,
        };
        let duplex = if mode.full_duplex {
            GMAC_CONTROL_FULL_DUPLEX
        } else {
            0
        };
        let control = (self.read_reg(GMAC_CONTROL)
            & !(GMAC_CONTROL_MII_PORT
                | GMAC_CONTROL_FAST_ETHERNET_SPEED
                | GMAC_CONTROL_FULL_DUPLEX))
            | speed
            | duplex;
        self.write_reg(GMAC_CONTROL, control);

        let mut flow = GMAC_FLOW_CONTROL_UNICAST_PAUSE;
        if mode.full_duplex {
            flow |= GMAC_FLOW_CONTROL_PAUSE_MASK;
        }
        if mode.pause_rx {
            flow |= GMAC_FLOW_CONTROL_RX_ENABLE;
        }
        if mode.pause_tx {
            flow |= GMAC_FLOW_CONTROL_TX_ENABLE;
        }
        self.write_reg(GMAC_FLOW_CONTROL, flow);
        self.write_reg(
            GMAC_CONTROL,
            control | GMAC_CONTROL_TX_ENABLE | GMAC_CONTROL_RX_ENABLE,
        );
        H::dma_barrier();
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
        self.ensure_started()?;
        if !self.link_state.is_up() {
            return Err(DevError::Again);
        }
        for _ in 0..RING_SIZE {
            let index = self.rx_index;
            H::dma_barrier();
            let status = unsafe { read_volatile(&self.rings.rx[index].status) };
            if status & DESC_OWN != 0 {
                return Err(DevError::Again);
            }

            self.rx_index = (index + 1) % RING_SIZE;
            let raw_len = ((status & RX_DESC_FRAME_LEN_MASK) >> RX_DESC_FRAME_LEN_SHIFT) as usize;
            let packet_len = (status & (RX_DESC_ERROR | RX_DESC_FIRST | RX_DESC_LAST)
                == (RX_DESC_FIRST | RX_DESC_LAST))
                .then(|| rx_packet_len(raw_len, NET_BUFFER_LEN))
                .flatten();

            let mut allocated = if packet_len.is_some() {
                self.net_pool.alloc_boxed()
            } else {
                None
            };
            let disposition = rx_descriptor_disposition(packet_len, allocated.is_some());
            let result = match disposition {
                RxDescriptorDisposition::Deliver(packet_len) => {
                    // Linux stmmac removes the four-byte FCS in software when
                    // GMAC automatic CRC stripping is disabled, as it is here.
                    let mut packet = allocated.take().expect("RX allocation checked above");
                    packet.set_packet_len(packet_len);
                    packet
                        .packet_mut()
                        .copy_from_slice(&self.buffers.rx[index][..packet_len]);
                    Ok(Some(packet.into_buf_ptr()))
                }
                RxDescriptorDisposition::DropInvalid => {
                    warn!("LS2K1000 GMAC0 dropped RX descriptor {index}: status={status:#010x}");
                    Ok(None)
                }
                RxDescriptorDisposition::DropNoMemory => Err(DevError::NoMemory),
            };

            debug_assert!(disposition.must_rearm());
            H::dma_barrier();
            unsafe { write_volatile(&mut self.rings.rx[index].status, DESC_OWN) };
            H::dma_barrier();
            self.write_reg(DMA_RX_POLL_DEMAND, 0);
            match result {
                Ok(Some(packet)) => return Ok(packet),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
        Err(DevError::Again)
    }

    fn transmit_one(&mut self, tx_buf: NetBufPtr) -> DevResult {
        // NetBufPtr transfers ownership to the driver. Recover the owning
        // NetBuf before any fallible gate so every error path returns it to
        // the pool through Drop.
        let tx_buf = unsafe { NetBuf::from_buf_ptr(tx_buf) };
        self.ensure_started()?;
        if !self.link_state.is_up() {
            return Err(DevError::Again);
        }
        let len = tx_buf.packet().len();
        if !(1..=NET_BUFFER_LEN).contains(&len) {
            return Err(DevError::InvalidParam);
        }
        self.reclaim_tx();
        let index = self.tx_ring.submit_index().ok_or(DevError::Again)?;
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
        if self.lifecycle.irq_active() {
            self.write_reg(
                DMA_INTERRUPT_ENABLE,
                self.read_reg(DMA_INTERRUPT_ENABLE) | DMA_IRQ_TX | DMA_STATUS_NIS,
            );
        }
        self.write_reg(DMA_TX_POLL_DEMAND, 0);
        let reserved = self.tx_ring.reserve();
        debug_assert_eq!(reserved, Some(index));
        Ok(())
    }

    fn reclaim_tx(&mut self) {
        while self.tx_ring.in_flight() != 0 {
            let index = self.tx_ring.clean_index();
            H::dma_barrier();
            let status = unsafe { read_volatile(&self.rings.tx[index].status) };
            if status & DESC_OWN != 0 {
                break;
            }
            if status & TX_DESC_ERROR != 0 {
                warn!("LS2K1000 GMAC0 TX descriptor error {index}: status={status:#010x}");
            }
            unsafe {
                write_volatile(
                    &mut self.rings.tx[index].status,
                    if index + 1 == RING_SIZE {
                        TX_DESC_END_RING
                    } else {
                        0
                    },
                );
                write_volatile(&mut self.rings.tx[index].length, 0);
            }
            let completed = self.tx_ring.complete_one();
            debug_assert!(completed);
        }
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
            let status = self.read_reg(DMA_STATUS) & DMA_STATUS_HANDLED;
            if status != 0 {
                self.write_reg(DMA_STATUS, status);
            }
            status
        };
        let events = dma_events_for_poll(irq_active, queued_events, hardware_status);
        if !irq_active && events != 0 {
            H::dma_barrier();
        }

        if events & DMA_STATUS_FBI != 0 {
            warn!("LS2K1000 GMAC0 fatal DMA bus error; resetting DMA queues");
            self.write_reg(DMA_INTERRUPT_ENABLE, 0);
            self.stop_tx_rx();
            self.lifecycle.stop();
            DMA_IRQ_ACTIVE.store(false, Ordering::Release);
            if self.reset_dma().is_ok() {
                self.rx_index = 0;
                self.tx_ring = TxRingState::new(RING_SIZE);
                if self.initialize_descriptors().is_ok() && self.initialize_mac().is_ok() {
                    if self.lifecycle.start() {
                        self.enable_hardware_interrupts();
                    }
                    self.refresh_phy_link(true, true);
                    return false;
                }
            }
            warn!("LS2K1000 GMAC0 DMA recovery failed; device remains stopped");
            DMA_IRQ_ACTIVE.store(false, Ordering::Release);
            return false;
        }
        if events & DMA_STATUS_RU != 0 {
            self.write_reg(DMA_RX_POLL_DEMAND, 0);
        }
        if events & DMA_STATUS_TU != 0 {
            self.write_reg(DMA_TX_POLL_DEMAND, 0);
        }
        if !irq_active {
            return link_changed;
        }

        H::dma_barrier();
        let rx_ready =
            unsafe { read_volatile(&self.rings.rx[self.rx_index].status) } & DESC_OWN == 0;
        let tx_ready = self.tx_ring.in_flight() != 0
            && unsafe { read_volatile(&self.rings.tx[self.tx_ring.clean_index()].status) }
                & DESC_OWN
                == 0;
        if rx_ready || tx_ready {
            DMA_IRQ_EVENTS.fetch_or(
                (if rx_ready { DMA_STATUS_RX } else { 0 })
                    | if tx_ready { DMA_STATUS_TX } else { 0 },
                Ordering::Release,
            );
            return true;
        }

        let mut enable = self.read_reg(DMA_INTERRUPT_ENABLE) | DMA_IRQ_RX | DMA_IRQ_ERRORS;
        if self.tx_ring.in_flight() != 0 {
            enable |= DMA_IRQ_TX;
        } else {
            enable &= !DMA_IRQ_TX;
        }
        enable |= DMA_STATUS_NIS;
        self.write_reg(DMA_INTERRUPT_ENABLE, enable);
        H::dma_barrier();

        let status = self.read_reg(DMA_STATUS) & DMA_STATUS_HANDLED;
        if status != 0 {
            self.write_reg(DMA_INTERRUPT_ENABLE, masked_irq_enable(enable, status));
            self.write_reg(DMA_STATUS, status);
            DMA_IRQ_EVENTS.fetch_or(status, Ordering::Release);
            return true;
        }
        link_changed
    }

    fn clause22_phy_sample(&self) -> PhySample {
        // BMSR link state is latch-low, so Linux genphy reads it twice.
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
            let gigabit_capable = if bmsr & PHY_BMSR_EXTENDED_STATUS != 0 {
                let Some(extended_status) = self.valid_mdio_read(PHY_EXTENDED_STATUS) else {
                    return PhySample::Unavailable;
                };
                clause22_gigabit_capable(bmsr, extended_status)
            } else {
                false
            };
            let (control_1000, status_1000) = if gigabit_capable {
                let Some(control_1000) = self.valid_mdio_read(PHY_CONTROL_1000) else {
                    return PhySample::Unavailable;
                };
                let Some(status_1000) = self.valid_mdio_read(PHY_STATUS_1000) else {
                    return PhySample::Unavailable;
                };
                (control_1000, status_1000)
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
        let now_ms = H::current_ms();
        if !force && now_ms < self.next_phy_poll_ms {
            return false;
        }
        self.next_phy_poll_ms = now_ms.saturating_add(PHY_POLL_INTERVAL_MS);
        let sample = self.clause22_phy_sample();
        if sample == PhySample::Unavailable {
            if !self.phy_unavailable_reported {
                warn!("LS2K1000 GMAC0 PHY became unavailable; preserving prior link state");
                self.phy_unavailable_reported = true;
            }
            self.phy_read_failed = true;
            if reapply_current {
                match self.link_state {
                    LinkState::Down => self.disable_mac_link(),
                    LinkState::Up(mode) => self.apply_link_mode(mode),
                }
            }
            return false;
        }
        if self.phy_read_failed {
            info!("LS2K1000 GMAC0 PHY MDIO access recovered");
            self.phy_read_failed = false;
        }

        match link_transition(self.link_state, sample) {
            LinkTransition::None => {
                if reapply_current {
                    match self.link_state {
                        LinkState::Down => self.disable_mac_link(),
                        LinkState::Up(mode) => self.apply_link_mode(mode),
                    }
                }
                false
            }
            LinkTransition::Down => {
                self.disable_mac_link();
                self.link_state = LinkState::Down;
                info!("LS2K1000 GMAC0 link down");
                true
            }
            LinkTransition::Up(mode) | LinkTransition::Reconfigure(mode) => {
                self.apply_link_mode(mode);
                self.link_state = LinkState::Up(mode);
                info!(
                    "LS2K1000 GMAC0 link up: {} Mbps, {}, pause_rx={}, pause_tx={}",
                    mode.speed_mbps,
                    if mode.full_duplex { "full" } else { "half" },
                    mode.pause_rx,
                    mode.pause_tx
                );
                true
            }
        }
    }

    fn valid_mdio_read(&self, register: u8) -> Option<u16> {
        self.mdio_read(self.phy_address, register)
            .filter(|value| *value != u16::MAX)
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
        if self.lifecycle.irq_registered() {
            self.irq
        } else {
            None
        }
    }
}

impl<H: DwmacHal> NetDriverOps for DwmacLs2k1000<H> {
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
            (unsafe { read_volatile(&self.rings.tx[index].status) } & DESC_OWN) == 0
        })
    }

    fn can_receive(&self) -> bool {
        if !self.lifecycle.started() || !self.link_state.is_up() {
            return false;
        }
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

/// Handles the singleton LS2K1000 GMAC DMA interrupt.
///
/// The hard handler only acknowledges hardware and publishes deferred work;
/// descriptor processing is performed by `NetDriverOps::poll_complete`.
pub fn handle_interrupt() {
    let irq_active = DMA_IRQ_ACTIVE.load(Ordering::Acquire);
    let base = DMA_MMIO_BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    let read = |offset: usize| unsafe {
        (base as *const u8)
            .add(offset)
            .cast::<u32>()
            .read_volatile()
    };
    let write = |offset: usize, value: u32| unsafe {
        (base as *mut u8)
            .add(offset)
            .cast::<u32>()
            .write_volatile(value)
    };
    // A DMA software reset also resets the GMAC interrupt mask. Its RGMII
    // status can therefore interrupt before DMA_IRQ_ACTIVE is published.
    // Service MAC-core read-to-clear sources independently of DMA state, as
    // Linux's stmmac host_irq_status path does, or that initialization-time
    // interrupt remains asserted forever while the handler keeps returning.
    let core_status = read(GMAC_INT_STATUS);
    let core_mask = read(GMAC_INT_MASK);
    write(GMAC_INT_MASK, core_mask | gmac_interrupt_mask(false));
    if core_status & GMAC_INT_STATUS_RGMII != 0 {
        read(GMAC_RGSMII_STATUS);
    }
    if core_status & GMAC_INT_STATUS_PMT != 0 {
        read(GMAC_PMT);
    }
    if core_status & GMAC_INT_STATUS_LPI != 0 {
        read(GMAC_LPI_CONTROL_STATUS);
    }

    let status = read(DMA_STATUS) & DMA_STATUS_HANDLED;
    core::sync::atomic::fence(Ordering::SeqCst);
    if !irq_active {
        return;
    }
    if status == 0 {
        return;
    }
    let enable = masked_irq_enable(read(DMA_INTERRUPT_ENABLE), status);
    write(DMA_INTERRUPT_ENABLE, enable);
    write(DMA_STATUS, status);
    core::sync::atomic::fence(Ordering::SeqCst);
    DMA_IRQ_EVENTS.fetch_or(status, Ordering::Release);
}
