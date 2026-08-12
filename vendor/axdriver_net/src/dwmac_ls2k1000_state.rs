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
    (queued_events | if irq_active { 0 } else { hardware_status }) & DMA_STATUS_HANDLED
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

// Linux dwmac1000 masks RGMII/PCS/timestamp sources (0x207) and services PMT
// in its host IRQ path. MOSS does not implement PMT/WOL handling, so mask that
// source too; otherwise an unacknowledged MAC-core source can hold the shared
// IRQ line asserted while the DMA status register reads zero.
const GMAC_INT_DEFAULT_MASK: u32 = 0x207;
const GMAC_INT_PMT: u32 = 1 << 3;

pub(crate) const fn gmac_interrupt_mask(has_pmt_handler: bool) -> u32 {
    GMAC_INT_DEFAULT_MASK | if has_pmt_handler { 0 } else { GMAC_INT_PMT }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn masks_every_unhandled_gmac_core_interrupt() {
        assert_eq!(gmac_interrupt_mask(false), 0x20f);
        assert_eq!(gmac_interrupt_mask(true), 0x207);
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
        assert_eq!(dma_events_for_poll(false, 0, u32::MAX), DMA_STATUS_HANDLED);
    }
}
