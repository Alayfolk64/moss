//! Pure JH7110 EQoS register policy used by the driver and host tests.

pub(crate) const DMA_STATUS_TI: u32 = 1 << 0;
pub(crate) const DMA_STATUS_TPS: u32 = 1 << 1;
pub(crate) const DMA_STATUS_TBU: u32 = 1 << 2;
pub(crate) const DMA_STATUS_RI: u32 = 1 << 6;
pub(crate) const DMA_STATUS_RBU: u32 = 1 << 7;
pub(crate) const DMA_STATUS_RPS: u32 = 1 << 8;
pub(crate) const DMA_STATUS_FBE: u32 = 1 << 12;
pub(crate) const DMA_STATUS_AIE: u32 = 1 << 14;
pub(crate) const DMA_STATUS_NIE: u32 = 1 << 15;
pub(crate) const DMA_STATUS_RX: u32 = DMA_STATUS_RI | DMA_STATUS_RBU | DMA_STATUS_RPS;
pub(crate) const DMA_STATUS_TX: u32 = DMA_STATUS_TI | DMA_STATUS_TBU | DMA_STATUS_TPS;
pub(crate) const DMA_STATUS_FATAL: u32 = DMA_STATUS_FBE;
pub(crate) const DMA_STATUS_HANDLED: u32 = (0x7 << 19) | 0xffff;
pub(crate) const DMA_IRQ_RX: u32 = DMA_STATUS_RI | DMA_STATUS_RBU | DMA_STATUS_RPS;
pub(crate) const DMA_IRQ_TX: u32 = DMA_STATUS_TI | DMA_STATUS_TBU;
pub(crate) const DMA_IRQ_ERRORS: u32 = DMA_STATUS_AIE | DMA_STATUS_FBE;
pub(crate) const DMA_IRQ_DEFAULT: u32 = DMA_STATUS_NIE | DMA_IRQ_ERRORS | DMA_IRQ_RX | DMA_IRQ_TX;

const MAC_PACKET_FILTER_PR: u32 = 1;
const MAC_PACKET_FILTER_HMC: u32 = 1 << 2;
const MAC_PACKET_FILTER_PM: u32 = 1 << 4;
const MAC_PACKET_FILTER_PCF_MASK: u32 = 3 << 6;
const MAC_PACKET_FILTER_HPF: u32 = 1 << 10;
const MAC_PACKET_FILTER_RA: u32 = 1 << 31;
const MAC_ADDRESS_HIGH_AE: u32 = 1 << 31;

/// Accept the primary perfect-match address, broadcasts, and all multicast.
///
/// `NetDriverOps` has no multicast-list callback, so passing multicast is the
/// safe fallback for IPv6/other multicast users without exposing unrelated
/// unicast traffic through permanent promiscuous mode.
pub(crate) const fn mac_packet_filter(current: u32) -> u32 {
    (current
        & !(MAC_PACKET_FILTER_PR
            | MAC_PACKET_FILTER_HMC
            | MAC_PACKET_FILTER_PM
            | MAC_PACKET_FILTER_PCF_MASK
            | MAC_PACKET_FILTER_HPF
            | MAC_PACKET_FILTER_RA))
        | MAC_PACKET_FILTER_PM
        | MAC_PACKET_FILTER_HPF
}

pub(crate) const fn mac_address_high(mac: [u8; 6]) -> u32 {
    u16::from_le_bytes([mac[4], mac[5]]) as u32 | MAC_ADDRESS_HIGH_AE
}

pub(crate) const fn dma_events_for_poll(
    irq_active: bool,
    queued_events: u32,
    hardware_status: u32,
) -> u32 {
    (queued_events | if irq_active { 0 } else { hardware_status }) & DMA_STATUS_HANDLED
}

/// A stopped TX process is actionable only while software still owns queued
/// work. EQoS also reports TPS together with TI/TBU after it consumes the
/// exclusive tail boundary and the ring becomes idle; resetting both queues
/// for that quiescent state can discard unrelated RX traffic.
pub(crate) const fn dma_requires_recovery(status: u32, tx_in_flight: usize) -> bool {
    status & DMA_STATUS_FBE != 0 || (status & DMA_STATUS_TPS != 0 && tx_in_flight != 0)
}

pub(crate) const fn masked_irq_enable(mut enable: u32, status: u32) -> u32 {
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

pub(crate) const fn valid_pbl(value: u32) -> bool {
    matches!(value, 1 | 2 | 4 | 8 | 16 | 32)
}

/// Convert the seven-cell `snps,blen` property to EQoS SYSBUS BLEN bits.
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

pub(crate) const fn dma_sysbus_mode(
    current: u32,
    write_limit: u8,
    read_limit: u8,
    burst_bits: u8,
    fixed_burst: bool,
) -> u32 {
    const FB: u32 = 1;
    const BLEN_MASK: u32 = 0x7f << 1;
    const EAME: u32 = 1 << 11;
    const RD_OSR_MASK: u32 = 0x0f << 16;
    const WR_OSR_MASK: u32 = 0x0f << 24;
    (current & !(FB | BLEN_MASK | RD_OSR_MASK | WR_OSR_MASK))
        | EAME
        | (burst_bits as u32 & BLEN_MASK)
        | (((read_limit & 0x0f) as u32) << 16)
        | (((write_limit & 0x0f) as u32) << 24)
        | if fixed_burst { FB } else { 0 }
}

/// Hardware FIFO fields encode 128-byte units as powers of two.
pub(crate) const fn hardware_fifo_depth(hw_feature1: u32, shift: u32) -> usize {
    128usize << ((hw_feature1 >> shift) & 0x1f)
}

pub(crate) const fn effective_fifo_depth(requested: usize, available: usize) -> usize {
    let requested = if requested == 0 { available } else { requested };
    if requested < available {
        requested
    } else {
        available
    }
}

const fn encoded_queue_size(fifo_depth: usize, maximum: u32) -> u32 {
    let queue_size = (fifo_depth / 256).saturating_sub(1);
    if queue_size > maximum as usize {
        maximum
    } else {
        queue_size as u32
    }
}

pub(crate) const fn mtl_tx_queue_mode(fifo_depth: usize, force_threshold: bool) -> u32 {
    // Clamp before shifting so a corrupt HW feature value cannot spill TQS
    // into adjacent MTL control fields.
    let queue_size = encoded_queue_size(fifo_depth, 0x1ff);
    (queue_size << 16) | (2 << 2) | if force_threshold { 3 << 4 } else { 1 << 1 }
}

pub(crate) const fn mtl_rx_queue_mode(fifo_depth: usize, force_threshold: bool) -> u32 {
    let queue_size = encoded_queue_size(fifo_depth, 0x3ff);
    (queue_size << 20) | if force_threshold { 3 } else { 1 << 5 }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TxClockInversion {
    pub(crate) speed_10: bool,
    pub(crate) speed_100: bool,
    pub(crate) speed_1000: bool,
}

impl TxClockInversion {
    pub(crate) const fn for_speed(self, speed_mbps: u16) -> Option<bool> {
        match speed_mbps {
            10 => Some(self.speed_10),
            100 => Some(self.speed_100),
            1_000 => Some(self.speed_1000),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irq_masking_is_per_queue_and_fatal_masks_all_work() {
        assert_eq!(
            masked_irq_enable(DMA_IRQ_DEFAULT, DMA_STATUS_RI),
            DMA_IRQ_DEFAULT & !DMA_IRQ_RX
        );
        assert_eq!(
            masked_irq_enable(DMA_IRQ_DEFAULT, DMA_STATUS_TI),
            DMA_IRQ_DEFAULT & !DMA_IRQ_TX
        );
        assert_eq!(
            masked_irq_enable(DMA_IRQ_DEFAULT, DMA_STATUS_FBE),
            DMA_IRQ_DEFAULT & !(DMA_IRQ_RX | DMA_IRQ_TX | DMA_IRQ_ERRORS)
        );
        assert_eq!(
            masked_irq_enable(DMA_IRQ_DEFAULT, DMA_STATUS_TPS),
            DMA_IRQ_DEFAULT & !DMA_IRQ_TX
        );
    }

    #[test]
    fn idle_tx_stop_does_not_reset_rx_and_active_tx_stop_does() {
        assert!(!dma_requires_recovery(
            DMA_STATUS_TI | DMA_STATUS_TPS | DMA_STATUS_TBU,
            0
        ));
        assert!(dma_requires_recovery(DMA_STATUS_TPS, 1));
        assert!(dma_requires_recovery(DMA_STATUS_FBE, 0));
    }

    #[test]
    fn polling_mode_reads_hardware_status() {
        assert_eq!(
            dma_events_for_poll(false, DMA_STATUS_TI, DMA_STATUS_RBU),
            DMA_STATUS_TI | DMA_STATUS_RBU
        );
        assert_eq!(
            dma_events_for_poll(true, DMA_STATUS_TI, DMA_STATUS_RBU),
            DMA_STATUS_TI
        );
    }

    #[test]
    fn parses_linux_dma_policy() {
        assert_eq!(axi_blen_mask(&[256, 128, 64, 32, 0, 0, 0]), Some(0xf0));
        assert_eq!(axi_blen_mask(&[256, 128, 64]), None);
        assert!(valid_pbl(16));
        assert!(!valid_pbl(64));
        let mode = dma_sysbus_mode(0, 15, 7, 0x7e, true);
        assert_eq!((mode >> 24) & 0xf, 15);
        assert_eq!((mode >> 16) & 0xf, 7);
        assert_ne!(mode & 1, 0);
    }

    #[test]
    fn fifo_sizes_are_bounded_by_hardware() {
        let feature = (4 << 6) | 3;
        assert_eq!(hardware_fifo_depth(feature, 6), 2_048);
        assert_eq!(hardware_fifo_depth(feature, 0), 1_024);
        assert_eq!(effective_fifo_depth(262_144, 1_024), 1_024);
        assert_eq!((mtl_tx_queue_mode(2_048, false) >> 16) & 0x1ff, 7);
        assert_eq!((mtl_rx_queue_mode(1_024, false) >> 20) & 0x3ff, 3);
        assert_eq!((mtl_tx_queue_mode(usize::MAX, false) >> 16) & 0x1ff, 0x1ff);
        assert_eq!((mtl_rx_queue_mode(usize::MAX, false) >> 20) & 0x3ff, 0x3ff);
    }

    #[test]
    fn motorcomm_phase_is_selected_per_negotiated_speed() {
        let phase = TxClockInversion {
            speed_10: false,
            speed_100: true,
            speed_1000: false,
        };
        assert_eq!(phase.for_speed(10), Some(false));
        assert_eq!(phase.for_speed(100), Some(true));
        assert_eq!(phase.for_speed(1_000), Some(false));
        assert_eq!(phase.for_speed(2_500), None);
    }

    #[test]
    fn receive_filter_uses_enabled_primary_address_without_promiscuous_mode() {
        let filter = mac_packet_filter(u32::MAX);
        assert_eq!(filter & MAC_PACKET_FILTER_PR, 0);
        assert_eq!(filter & MAC_PACKET_FILTER_RA, 0);
        assert_ne!(filter & MAC_PACKET_FILTER_PM, 0);
        assert_ne!(filter & MAC_PACKET_FILTER_HPF, 0);
        assert_eq!(mac_address_high([0, 1, 2, 3, 4, 5]), 0x8000_0504);
    }
}
