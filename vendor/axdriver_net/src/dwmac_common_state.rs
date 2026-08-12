//! Pure state shared by Synopsys DWMAC network drivers.
//!
//! Keeping PHY negotiation and queue ownership independent from MMIO makes
//! the rules usable by both the LS2K1000 DWMAC 3.x and JH7110 EQoS 5.x
//! drivers, and lets host-side tests cover failure paths.

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RxDescriptorDisposition {
    Deliver(usize),
    DropInvalid,
    DropNoMemory,
}

impl RxDescriptorDisposition {
    /// Every CPU-owned descriptor must be returned to DMA, including drops.
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

pub(crate) const fn clause22_gigabit_capable(bmsr: u16, extended_status: u16) -> bool {
    bmsr & BMSR_EXTENDED_STATUS != 0 && extended_status & EXTENDED_STATUS_GIGABIT_MASK != 0
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

/// Resolve link mode from the standard Clause 22 registers used by genphy.
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

/// Probe and IRQ registration are separate from first-use DMA activation.
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

    pub(crate) fn register_irq(&mut self) -> bool {
        self.irq_registered = true;
        self.irq_active()
    }

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

    pub(crate) const fn clean_index(&self) -> usize {
        self.clean
    }

    /// Exclusive producer boundary consumed by the DMA TX tail register.
    ///
    /// This remains meaningful when the ring is full, whereas `submit_index`
    /// deliberately returns `None` in that state.  Recovery paths must publish
    /// this boundary rather than the oldest descriptor awaiting completion.
    #[cfg(any(feature = "dwmac-jh7110", test))]
    pub(crate) const fn tail_index(&self) -> usize {
        self.submit
    }

    pub(crate) const fn in_flight(&self) -> usize {
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

/// Descriptor immediately before a ring's next-consumer index.
///
/// The DWMAC RX tail register names the last descriptor returned to DMA, not
/// the next descriptor that software will inspect.
#[cfg(any(feature = "dwmac-jh7110", test))]
pub(crate) fn previous_ring_index(next: usize, capacity: usize) -> usize {
    assert!(capacity != 0);
    debug_assert!(next < capacity);
    if next == 0 { capacity - 1 } else { next - 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_defers_dma_until_first_use() {
        let mut lifecycle = DmaLifecycle::new();
        assert!(!lifecycle.register_irq());
        assert!(lifecycle.start());
        lifecycle.stop();
        assert!(!lifecycle.started());
        assert!(lifecycle.irq_registered());
    }

    #[test]
    fn rx_drop_is_always_rearmed() {
        assert!(rx_descriptor_disposition(Some(64), false).must_rearm());
        assert!(rx_descriptor_disposition(None, true).must_rearm());
    }

    #[test]
    fn clause22_resolves_gigabit_and_forced_modes() {
        assert_eq!(
            clause22_link_mode(0x1000, 0x0124, 0x05e0, 0x05e0, 0x0200, 0x0800),
            Some(PhyLinkMode {
                speed_mbps: 1_000,
                full_duplex: true,
                pause_rx: true,
                pause_tx: true,
            })
        );
        assert_eq!(
            clause22_link_mode(0x2100, 0x0004, 0, 0, 0, 0)
                .unwrap()
                .speed_mbps,
            100
        );
        assert!(clause22_gigabit_capable(1 << 8, 1 << 13));
    }

    #[test]
    fn link_state_keeps_last_good_sample_on_mdio_error() {
        let mode = PhyLinkMode {
            speed_mbps: 1_000,
            full_duplex: true,
            pause_rx: false,
            pause_tx: false,
        };
        assert_eq!(
            link_transition(LinkState::Up(mode), PhySample::Unavailable),
            LinkTransition::None
        );
        assert_eq!(
            link_transition(LinkState::Up(mode), PhySample::Down),
            LinkTransition::Down
        );
    }

    #[test]
    fn tx_ring_wraps_and_enforces_capacity() {
        let mut ring = TxRingState::new(2);
        assert_eq!(ring.reserve(), Some(0));
        assert_eq!(ring.tail_index(), 1);
        assert_eq!(ring.reserve(), Some(1));
        assert_eq!(ring.tail_index(), 0);
        assert_eq!(ring.reserve(), None);
        assert!(ring.complete_one());
        assert_eq!(ring.reserve(), Some(0));
        assert_eq!(ring.tail_index(), 1);
    }

    #[test]
    fn rx_tail_is_the_descriptor_before_the_next_consumer() {
        assert_eq!(previous_ring_index(0, 64), 63);
        assert_eq!(previous_ring_index(17, 64), 16);
    }
}
