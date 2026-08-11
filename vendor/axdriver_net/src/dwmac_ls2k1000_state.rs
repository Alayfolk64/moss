//! Pure DWMAC interrupt and TX-ring state shared by the driver and host tests.

pub(crate) const DMA_STATUS_TI: u32 = 1 << 0;
pub(crate) const DMA_STATUS_TPS: u32 = 1 << 1;
pub(crate) const DMA_STATUS_TU: u32 = 1 << 2;
pub(crate) const DMA_STATUS_OVF: u32 = 1 << 4;
pub(crate) const DMA_STATUS_UNF: u32 = 1 << 5;
pub(crate) const DMA_STATUS_RI: u32 = 1 << 6;
pub(crate) const DMA_STATUS_RU: u32 = 1 << 7;
pub(crate) const DMA_STATUS_RPS: u32 = 1 << 8;
pub(crate) const DMA_STATUS_FBI: u32 = 1 << 13;
pub(crate) const DMA_STATUS_AIS: u32 = 1 << 15;
pub(crate) const DMA_STATUS_NIS: u32 = 1 << 16;
pub(crate) const DMA_STATUS_RX: u32 =
    DMA_STATUS_RI | DMA_STATUS_RU | DMA_STATUS_RPS | DMA_STATUS_OVF;
pub(crate) const DMA_STATUS_TX: u32 =
    DMA_STATUS_TI | DMA_STATUS_TU | DMA_STATUS_TPS | DMA_STATUS_UNF;
pub(crate) const DMA_STATUS_FATAL: u32 = DMA_STATUS_FBI;
// Linux dwmac_lib clears CSR5[16:0] from the value it read. Include status
// sources that are normally masked too, otherwise a latent abnormal source can
// keep the summary bit asserted after the hard handler acknowledges it.
pub(crate) const DMA_STATUS_HANDLED: u32 = 0x1ffff;
// Match Linux dwmac_dma.h's DMA_INTR_DEFAULT_{RX,TX} and
// DMA_INTR_ABNORMAL. Status bits such as RU/RPS/TU/TPS are acknowledged when
// observed, but are not enabled as independent interrupt sources.
pub(crate) const DMA_IRQ_RX: u32 = DMA_STATUS_RI;
pub(crate) const DMA_IRQ_TX: u32 = DMA_STATUS_TI;
pub(crate) const DMA_IRQ_ERRORS: u32 = DMA_STATUS_AIS | DMA_STATUS_FBI | DMA_STATUS_UNF;
pub(crate) const DMA_IRQ_DEFAULT: u32 = DMA_IRQ_RX | DMA_IRQ_TX | DMA_IRQ_ERRORS | DMA_STATUS_NIS;

pub(crate) const fn dma_events_for_poll(
    irq_active: bool,
    queued_events: u32,
    hardware_status: u32,
) -> u32 {
    (queued_events
        | if irq_active {
            0
        } else {
            hardware_status
        })
        & DMA_STATUS_HANDLED
}

pub(crate) fn masked_irq_enable(mut enable: u32, status: u32) -> u32 {
    if status & DMA_STATUS_RX != 0 {
        enable &= !DMA_IRQ_RX;
    }
    if status & DMA_STATUS_TX != 0 {
        enable &= !DMA_IRQ_TX;
    }
    if status & DMA_STATUS_FATAL != 0 {
        enable &= !(DMA_IRQ_RX | DMA_IRQ_TX | DMA_IRQ_ERRORS);
    }
    enable
}

const ETHERNET_FCS_LEN: usize = 4;

/// DWMAC enhanced descriptors include the Ethernet FCS in the reported frame
/// length when GMAC automatic CRC stripping is disabled. Linux stmmac removes
/// those four bytes before publishing the packet to the network stack.
pub(crate) fn rx_packet_len(descriptor_len: usize, max_packet_len: usize) -> Option<usize> {
    descriptor_len
        .checked_sub(ETHERNET_FCS_LEN)
        .filter(|len| (1..=max_packet_len).contains(len))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RxDescriptorDisposition {
    Deliver(usize),
    DropInvalid,
    DropNoMemory,
}

impl RxDescriptorDisposition {
    /// Every descriptor consumed by the CPU must be returned to DMA, including
    /// invalid frames and temporary network-buffer allocation failures.
    pub(crate) const fn must_rearm(self) -> bool {
        true
    }
}

pub(crate) const fn rx_descriptor_disposition(
    packet_len: Option<usize>,
    buffer_available: bool,
) -> RxDescriptorDisposition {
    match (packet_len, buffer_available) {
        (Some(len), true) => RxDescriptorDisposition::Deliver(len),
        (Some(_), false) => RxDescriptorDisposition::DropNoMemory,
        (None, _) => RxDescriptorDisposition::DropInvalid,
    }
}

// Linux dwmac1000 masks RGMII/PCS/timestamp sources (0x207) and services PMT
// in its host IRQ path. MOSS does not implement PMT/WOL handling, so mask that
// source too; otherwise an unacknowledged MAC-core source can hold the shared
// IRQ line asserted while the DMA status register reads zero.
const GMAC_INT_DEFAULT_MASK: u32 = 0x207;
const GMAC_INT_PMT: u32 = 1 << 3;

pub(crate) const fn gmac_interrupt_mask(has_pmt_handler: bool) -> u32 {
    GMAC_INT_DEFAULT_MASK | if has_pmt_handler { 0 } else { GMAC_INT_PMT }
}

const BMCR_SPEED1000: u16 = 1 << 6;
const BMCR_FULL_DUPLEX: u16 = 1 << 8;
const BMCR_AN_ENABLE: u16 = 1 << 12;
const BMCR_SPEED100: u16 = 1 << 13;
const BMSR_LINK: u16 = 1 << 2;
const BMSR_AN_COMPLETE: u16 = 1 << 5;
const BMSR_EXTENDED_STATUS: u16 = 1 << 8;
const EXTENDED_STATUS_GIGABIT_MASK: u16 = 0xf000;
const ADVERTISE_10_HALF: u16 = 1 << 5;
const ADVERTISE_10_FULL: u16 = 1 << 6;
const ADVERTISE_100_HALF: u16 = 1 << 7;
const ADVERTISE_100_FULL: u16 = 1 << 8;
const ADVERTISE_1000_HALF: u16 = 1 << 8;
const ADVERTISE_1000_FULL: u16 = 1 << 9;
const LPA_1000_HALF: u16 = 1 << 10;
const LPA_1000_FULL: u16 = 1 << 11;
const ADVERTISE_PAUSE_CAP: u16 = 1 << 10;
const ADVERTISE_PAUSE_ASYM: u16 = 1 << 11;

pub(crate) const fn clause22_gigabit_capable(bmsr: u16, extended_status: u16) -> bool {
    bmsr & BMSR_EXTENDED_STATUS != 0
        && extended_status & EXTENDED_STATUS_GIGABIT_MASK != 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyLinkMode {
    pub(crate) speed_mbps: u16,
    pub(crate) full_duplex: bool,
    pub(crate) pause_rx: bool,
    pub(crate) pause_tx: bool,
}

const fn resolved_pause(advertise: u16, link_partner: u16) -> (bool, bool) {
    let common = advertise & link_partner;
    if common & ADVERTISE_PAUSE_CAP != 0 {
        (true, true)
    } else if common & ADVERTISE_PAUSE_ASYM != 0 {
        if advertise & ADVERTISE_PAUSE_CAP != 0 {
            (true, false)
        } else if link_partner & ADVERTISE_PAUSE_CAP != 0 {
            (false, true)
        } else {
            (false, false)
        }
    } else {
        (false, false)
    }
}

const fn negotiated_mode(
    speed_mbps: u16,
    full_duplex: bool,
    advertise: u16,
    link_partner: u16,
) -> PhyLinkMode {
    let (pause_rx, pause_tx) = if full_duplex {
        resolved_pause(advertise, link_partner)
    } else {
        (false, false)
    };
    PhyLinkMode {
        speed_mbps,
        full_duplex,
        pause_rx,
        pause_tx,
    }
}

/// Resolve a generic Clause 22 PHY exactly from the standard BMCR/BMSR,
/// advertisement, link-partner, and 1000BASE-T registers used by Linux genphy.
pub(crate) const fn clause22_link_mode(
    bmcr: u16,
    bmsr: u16,
    advertise: u16,
    link_partner: u16,
    control_1000: u16,
    status_1000: u16,
) -> Option<PhyLinkMode> {
    if bmsr & BMSR_LINK == 0 {
        return None;
    }
    if bmcr & BMCR_AN_ENABLE == 0 {
        return Some(PhyLinkMode {
            speed_mbps: if bmcr & BMCR_SPEED1000 != 0 {
                1_000
            } else if bmcr & BMCR_SPEED100 != 0 {
                100
            } else {
                10
            },
            full_duplex: bmcr & BMCR_FULL_DUPLEX != 0,
            pause_rx: false,
            pause_tx: false,
        });
    }
    if bmsr & BMSR_AN_COMPLETE == 0 {
        return None;
    }

    if control_1000 & ADVERTISE_1000_FULL != 0 && status_1000 & LPA_1000_FULL != 0 {
        return Some(negotiated_mode(1_000, true, advertise, link_partner));
    }
    if control_1000 & ADVERTISE_1000_HALF != 0 && status_1000 & LPA_1000_HALF != 0 {
        return Some(negotiated_mode(1_000, false, advertise, link_partner));
    }

    let common = advertise & link_partner;
    if common & ADVERTISE_100_FULL != 0 {
        Some(negotiated_mode(100, true, advertise, link_partner))
    } else if common & ADVERTISE_100_HALF != 0 {
        Some(negotiated_mode(100, false, advertise, link_partner))
    } else if common & ADVERTISE_10_FULL != 0 {
        Some(negotiated_mode(10, true, advertise, link_partner))
    } else if common & ADVERTISE_10_HALF != 0 {
        Some(negotiated_mode(10, false, advertise, link_partner))
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PhySample {
    Unavailable,
    Down,
    Up(PhyLinkMode),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinkState {
    Down,
    Up(PhyLinkMode),
}

impl LinkState {
    pub(crate) const fn is_up(self) -> bool {
        matches!(self, Self::Up(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinkTransition {
    None,
    Down,
    Up(PhyLinkMode),
    Reconfigure(PhyLinkMode),
}

pub(crate) fn link_transition(current: LinkState, sample: PhySample) -> LinkTransition {
    match (current, sample) {
        (_, PhySample::Unavailable) | (LinkState::Down, PhySample::Down) => LinkTransition::None,
        (LinkState::Up(_), PhySample::Down) => LinkTransition::Down,
        (LinkState::Down, PhySample::Up(mode)) => LinkTransition::Up(mode),
        (LinkState::Up(current), PhySample::Up(sample)) if current == sample => {
            LinkTransition::None
        }
        (LinkState::Up(_), PhySample::Up(mode)) => LinkTransition::Reconfigure(mode),
    }
}

/// Convert the seven-cell Linux `snps,blen` array into DWMAC AXI BLEN bits.
/// Zero cells are placeholders; non-zero cells must be supported powers of two.
pub(crate) fn axi_blen_mask(values: &[u32]) -> Option<u8> {
    if values.len() != 7 {
        return None;
    }
    let mut mask = 0_u8;
    for &burst in values {
        if burst == 0 {
            continue;
        }
        if !(4..=256).contains(&burst) || !burst.is_power_of_two() {
            return None;
        }
        mask |= ((burst >> 2) << 1) as u8;
    }
    Some(mask)
}

pub(crate) const fn valid_pbl(value: u32) -> bool {
    matches!(value, 1 | 2 | 4 | 8 | 16 | 32)
}

const GMAC_CONTROL_JABBER_DISABLE: u32 = 0x0040_0000;
const GMAC_CONTROL_FRAME_BURST: u32 = 0x0020_0000;
const GMAC_CONTROL_DISABLE_CARRIER_SENSE: u32 = 0x0001_0000;
const GMAC_CONTROL_MII_PORT: u32 = 0x0000_8000;
const GMAC_FRAME_FILTER_HASH_OR_PERFECT: u32 = 0x0000_0400;

pub(crate) const fn linux_mac_core_config(current: u32) -> u32 {
    current
        | GMAC_CONTROL_JABBER_DISABLE
        | GMAC_CONTROL_FRAME_BURST
        | GMAC_CONTROL_DISABLE_CARRIER_SENSE
        | GMAC_CONTROL_MII_PORT
}

pub(crate) const fn linux_mac_frame_filter() -> u32 {
    GMAC_FRAME_FILTER_HASH_OR_PERFECT
}

const DMA_BUS_MODE_PBL_MASK: u32 = 0x3f << 8;
const DMA_BUS_MODE_ALT_DESCRIPTOR_SIZE: u32 = 1 << 7;
const DMA_BUS_MODE_FIXED_BURST: u32 = 1 << 16;
const DMA_BUS_MODE_RX_PBL_MASK: u32 = 0x3f << 17;
const DMA_BUS_MODE_SEPARATE_PBL: u32 = 1 << 23;
const DMA_BUS_MODE_PBL_X8: u32 = 1 << 24;
const DMA_BUS_MODE_ADDRESS_ALIGNED: u32 = 1 << 25;
const DMA_BUS_MODE_MIXED_BURST: u32 = 1 << 26;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DmaBusConfig {
    pub(crate) tx_pbl: u32,
    pub(crate) rx_pbl: u32,
    pub(crate) pbl_x8: bool,
    pub(crate) fixed_burst: bool,
    pub(crate) mixed_burst: bool,
    pub(crate) address_aligned: bool,
    pub(crate) extended_desc: bool,
}

pub(crate) const fn linux_dma_bus_mode_config(current: u32, config: DmaBusConfig) -> u32 {
    let controlled = DMA_BUS_MODE_PBL_MASK
        | DMA_BUS_MODE_ALT_DESCRIPTOR_SIZE
        | DMA_BUS_MODE_FIXED_BURST
        | DMA_BUS_MODE_RX_PBL_MASK
        | DMA_BUS_MODE_SEPARATE_PBL
        | DMA_BUS_MODE_PBL_X8
        | DMA_BUS_MODE_ADDRESS_ALIGNED
        | DMA_BUS_MODE_MIXED_BURST;
    (current & !controlled)
        | ((config.tx_pbl & 0x3f) << 8)
        | ((config.rx_pbl & 0x3f) << 17)
        | DMA_BUS_MODE_SEPARATE_PBL
        | if config.pbl_x8 {
            DMA_BUS_MODE_PBL_X8
        } else {
            0
        }
        | if config.fixed_burst {
            DMA_BUS_MODE_FIXED_BURST
        } else {
            0
        }
        | if config.mixed_burst {
            DMA_BUS_MODE_MIXED_BURST
        } else {
            0
        }
        | if config.address_aligned {
            DMA_BUS_MODE_ADDRESS_ALIGNED
        } else {
            0
        }
        | if config.extended_desc {
            DMA_BUS_MODE_ALT_DESCRIPTOR_SIZE
        } else {
            0
        }
}

pub(crate) const fn linux_axi_bus_mode_config(
    current: u32,
    write_outstanding_limit: u32,
    read_outstanding_limit: u32,
    burst_register_bits: u32,
) -> u32 {
    const WRITE_LIMIT_MASK: u32 = 0xf << 20;
    const READ_LIMIT_MASK: u32 = 0xf << 16;
    const BURST_MASK: u32 = 0x7f << 1;
    (current & !(WRITE_LIMIT_MASK | READ_LIMIT_MASK | BURST_MASK))
        | ((write_outstanding_limit & 0xf) << 20)
        | ((read_outstanding_limit & 0xf) << 16)
        | (burst_register_bits & BURST_MASK)
}

/// Linux stmmac keeps the DMA engine stopped after probe and starts it from
/// ndo_open.  MOSS has no explicit netdev-open callback, so the first queue
/// operation is the equivalent activation point.  IRQ registration and DMA
/// activation remain separate: registering a handler must not touch a live
/// DMA engine during early boot.
#[derive(Debug, Default)]
pub(crate) struct DmaLifecycle {
    started: bool,
    irq_registered: bool,
}

impl DmaLifecycle {
    pub(crate) const fn new() -> Self {
        Self {
            started: false,
            irq_registered: false,
        }
    }

    /// Records a registered platform handler and reports whether hardware
    /// interrupts may be enabled immediately.
    pub(crate) fn register_irq(&mut self) -> bool {
        self.irq_registered = true;
        self.irq_active()
    }

    /// Marks the DMA engine started and reports whether the registered IRQ
    /// should now be unmasked in hardware.
    pub(crate) fn start(&mut self) -> bool {
        self.started = true;
        self.irq_active()
    }

    pub(crate) fn stop(&mut self) {
        self.started = false;
    }

    pub(crate) const fn started(&self) -> bool {
        self.started
    }

    pub(crate) const fn irq_registered(&self) -> bool {
        self.irq_registered
    }

    pub(crate) const fn irq_active(&self) -> bool {
        self.started && self.irq_registered
    }
}

#[derive(Debug)]
pub(crate) struct TxRingState {
    submit: usize,
    clean: usize,
    in_flight: usize,
    capacity: usize,
}

impl TxRingState {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity != 0);
        Self {
            submit: 0,
            clean: 0,
            in_flight: 0,
            capacity,
        }
    }

    pub(crate) fn submit_index(&self) -> Option<usize> {
        (self.in_flight < self.capacity).then_some(self.submit)
    }

    pub(crate) fn reserve(&mut self) -> Option<usize> {
        let index = self.submit_index()?;
        self.submit = (index + 1) % self.capacity;
        self.in_flight += 1;
        Some(index)
    }

    pub(crate) fn clean_index(&self) -> usize {
        self.clean
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight
    }

    pub(crate) fn complete_one(&mut self) -> bool {
        if self.in_flight == 0 {
            return false;
        }
        self.clean = (self.clean + 1) % self.capacity;
        self.in_flight -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_keeps_dma_and_interrupts_stopped() {
        let lifecycle = DmaLifecycle::new();

        assert!(!lifecycle.started());
        assert!(!lifecycle.irq_registered());
        assert!(!lifecycle.irq_active());
    }

    #[test]
    fn registering_irq_does_not_start_dma() {
        let mut lifecycle = DmaLifecycle::new();

        assert!(!lifecycle.register_irq());
        assert!(lifecycle.irq_registered());
        assert!(!lifecycle.started());
        assert!(!lifecycle.irq_active());
    }

    #[test]
    fn first_open_starts_dma_and_activates_registered_irq() {
        let mut lifecycle = DmaLifecycle::new();
        lifecycle.register_irq();

        assert!(lifecycle.start());
        assert!(lifecycle.started());
        assert!(lifecycle.irq_active());
    }

    #[test]
    fn failed_recovery_returns_to_stopped_state_without_losing_irq_registration() {
        let mut lifecycle = DmaLifecycle::new();
        lifecycle.register_irq();
        lifecycle.start();

        lifecycle.stop();

        assert!(!lifecycle.started());
        assert!(lifecycle.irq_registered());
        assert!(!lifecycle.irq_active());
    }

    #[test]
    fn linux_default_irq_mask_enables_only_normal_and_selected_abnormal_sources() {
        assert_eq!(
            DMA_IRQ_DEFAULT,
            DMA_STATUS_TI
                | DMA_STATUS_RI
                | DMA_STATUS_NIS
                | DMA_STATUS_AIS
                | DMA_STATUS_FBI
                | DMA_STATUS_UNF
        );
        assert_eq!(DMA_STATUS_HANDLED, 0x1ffff);
    }

    #[test]
    fn rx_length_excludes_fcs_when_automatic_crc_stripping_is_disabled() {
        assert_eq!(rx_packet_len(64, 1526), Some(60));
        assert_eq!(rx_packet_len(4, 1526), None);
        assert_eq!(rx_packet_len(3, 1526), None);
        assert_eq!(rx_packet_len(1530, 1526), Some(1526));
        assert_eq!(rx_packet_len(1531, 1526), None);
    }

    #[test]
    fn rx_pool_exhaustion_still_returns_descriptor_to_dma() {
        let disposition = rx_descriptor_disposition(Some(60), false);

        assert_eq!(disposition, RxDescriptorDisposition::DropNoMemory);
        assert!(disposition.must_rearm());
    }

    #[test]
    fn masks_every_unhandled_gmac_core_interrupt() {
        assert_eq!(gmac_interrupt_mask(false), 0x20f);
        assert_eq!(gmac_interrupt_mask(true), 0x207);
    }

    #[test]
    fn clause22_autoneg_selects_highest_common_mode() {
        assert_eq!(
            clause22_link_mode(0x1000, 0x0024, 0x01e0, 0x01e0, 0x0200, 0x0800),
            Some(PhyLinkMode {
                speed_mbps: 1_000,
                full_duplex: true,
                pause_rx: false,
                pause_tx: false,
            })
        );
        assert_eq!(
            clause22_link_mode(0x1000, 0x0024, 0x01e0, 0x00e0, 0, 0),
            Some(PhyLinkMode {
                speed_mbps: 100,
                full_duplex: false,
                pause_rx: false,
                pause_tx: false,
            })
        );
    }

    #[test]
    fn clause22_rejects_down_or_incomplete_link_and_supports_forced_mode() {
        assert_eq!(clause22_link_mode(0x1000, 0x0020, 0, 0, 0, 0), None);
        assert_eq!(clause22_link_mode(0x1000, 0x0004, 0, 0, 0, 0), None);
        assert_eq!(
            clause22_link_mode(0x2100, 0x0004, 0, 0, 0, 0),
            Some(PhyLinkMode {
                speed_mbps: 100,
                full_duplex: true,
                pause_rx: false,
                pause_tx: false,
            })
        );
    }

    #[test]
    fn clause22_resolves_symmetric_and_asymmetric_pause() {
        let symmetric = clause22_link_mode(0x1000, 0x0024, 0x05e0, 0x05e0, 0, 0).unwrap();
        assert!(symmetric.pause_rx);
        assert!(symmetric.pause_tx);

        let rx_only = clause22_link_mode(0x1000, 0x0024, 0x0de0, 0x09e0, 0, 0).unwrap();
        assert!(rx_only.pause_rx);
        assert!(!rx_only.pause_tx);
    }

    #[test]
    fn clause22_reads_gigabit_registers_only_for_a_gigabit_capable_phy() {
        assert!(!clause22_gigabit_capable(0, u16::MAX));
        assert!(!clause22_gigabit_capable(1 << 8, 0));
        assert!(clause22_gigabit_capable(1 << 8, 1 << 13));
        assert!(clause22_gigabit_capable(1 << 8, 1 << 15));
    }

    #[test]
    fn link_transition_handles_down_reconnect_mode_change_and_mdio_error() {
        let gigabit = PhyLinkMode {
            speed_mbps: 1_000,
            full_duplex: true,
            pause_rx: true,
            pause_tx: true,
        };
        let fast = PhyLinkMode {
            speed_mbps: 100,
            full_duplex: true,
            pause_rx: false,
            pause_tx: false,
        };

        assert_eq!(
            link_transition(LinkState::Down, PhySample::Up(gigabit)),
            LinkTransition::Up(gigabit)
        );
        assert_eq!(
            link_transition(LinkState::Up(gigabit), PhySample::Down),
            LinkTransition::Down
        );
        assert_eq!(
            link_transition(LinkState::Up(gigabit), PhySample::Up(fast)),
            LinkTransition::Reconfigure(fast)
        );
        assert_eq!(
            link_transition(LinkState::Up(gigabit), PhySample::Unavailable),
            LinkTransition::None
        );
    }

    #[test]
    fn validates_linux_axi_blen_and_pbl_contract() {
        assert_eq!(axi_blen_mask(&[4, 8, 16, 32, 0, 0, 0]), Some(0x1e));
        assert_eq!(axi_blen_mask(&[4, 8, 16, 32]), None);
        assert_eq!(axi_blen_mask(&[4, 8, 16, 32, 64, 128, 3]), None);
        for value in [1, 2, 4, 8, 16, 32] {
            assert!(valid_pbl(value));
        }
        for value in [0, 3, 24, 64] {
            assert!(!valid_pbl(value));
        }
    }

    #[test]
    fn mac_core_and_filter_defaults_match_linux_dwmac1000() {
        assert_eq!(linux_mac_core_config(0), 0x0061_8000);
        assert_eq!(linux_mac_core_config(1 << 3), 0x0061_8008);
        assert_eq!(linux_mac_frame_filter(), 0x0000_0400);
    }

    #[test]
    fn dma_bus_mode_uses_32_byte_enhanced_descriptors_and_symmetric_pbl32() {
        let configured = linux_dma_bus_mode_config(
            u32::MAX,
            DmaBusConfig {
                tx_pbl: 32,
                rx_pbl: 32,
                pbl_x8: true,
                fixed_burst: false,
                mixed_burst: true,
                address_aligned: false,
                extended_desc: true,
            },
        );

        assert_eq!((configured >> 8) & 0x3f, 32);
        assert_eq!((configured >> 17) & 0x3f, 32);
        assert_ne!(configured & (1 << 23), 0); // separate RX/TX PBL
        assert_ne!(configured & (1 << 24), 0); // PBL x8 mode
        assert_ne!(configured & (1 << 26), 0); // mixed burst
        assert_ne!(configured & (1 << 7), 0); // ATDS: eight-word descriptor stride
    }

    #[test]
    fn linux_default_dma_config_is_pbl8_x8_without_forced_burst_mode() {
        let configured = linux_dma_bus_mode_config(
            0,
            DmaBusConfig {
                tx_pbl: 8,
                rx_pbl: 8,
                pbl_x8: true,
                fixed_burst: false,
                mixed_burst: false,
                address_aligned: false,
                extended_desc: true,
            },
        );

        assert_eq!((configured >> 8) & 0x3f, 8);
        assert_eq!((configured >> 17) & 0x3f, 8);
        assert_eq!(configured & (1 << 16), 0);
        assert_eq!(configured & (1 << 26), 0);
        assert_ne!(configured & (1 << 7), 0);
    }

    #[test]
    fn axi_config_preserves_read_only_undef_bit_and_replaces_dt_fields() {
        let configured = linux_axi_bus_mode_config(u32::MAX, 1, 2, 0x2a);

        assert_eq!(configured & 1, 1);
        assert_eq!((configured >> 20) & 0xf, 1);
        assert_eq!((configured >> 16) & 0xf, 2);
        assert_eq!(configured & 0xfe, 0x2a);
    }

    #[test]
    fn irq_mask_only_disables_asserted_queue() {
        assert_eq!(
            masked_irq_enable(DMA_IRQ_DEFAULT, DMA_STATUS_RI),
            DMA_IRQ_DEFAULT & !DMA_IRQ_RX
        );
        assert_eq!(
            masked_irq_enable(DMA_IRQ_DEFAULT, DMA_STATUS_TI),
            DMA_IRQ_DEFAULT & !DMA_IRQ_TX
        );
    }

    #[test]
    fn fatal_irq_masks_all_dma_sources() {
        assert_eq!(
            masked_irq_enable(DMA_IRQ_DEFAULT, DMA_STATUS_FBI),
            DMA_IRQ_DEFAULT & !(DMA_IRQ_RX | DMA_IRQ_TX | DMA_IRQ_ERRORS)
        );
    }

    #[test]
    fn polling_mode_merges_hardware_dma_status_into_deferred_events() {
        assert_eq!(
            dma_events_for_poll(false, DMA_STATUS_RI, DMA_STATUS_FBI),
            DMA_STATUS_RI | DMA_STATUS_FBI
        );
        assert_eq!(
            dma_events_for_poll(true, DMA_STATUS_RI, DMA_STATUS_FBI),
            DMA_STATUS_RI
        );
        assert_eq!(
            dma_events_for_poll(false, 0, u32::MAX),
            DMA_STATUS_HANDLED
        );
    }

    #[test]
    fn tx_ring_wraps_and_enforces_capacity() {
        let mut ring = TxRingState::new(4);
        for expected in 0..4 {
            assert_eq!(ring.reserve(), Some(expected));
        }
        assert_eq!(ring.reserve(), None);
        assert_eq!(ring.clean_index(), 0);
        assert!(ring.complete_one());
        assert_eq!(ring.clean_index(), 1);
        assert_eq!(ring.reserve(), Some(0));
    }

    #[test]
    fn tx_ring_rejects_spurious_completion() {
        let mut ring = TxRingState::new(2);
        assert!(!ring.complete_one());
        assert_eq!(ring.reserve(), Some(0));
        assert!(ring.complete_one());
        assert!(!ring.complete_one());
    }
}
