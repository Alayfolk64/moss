//! Synopsys DWMAC 5.20 register offsets and bit definitions.

pub(super) const MAC_CONFIGURATION: usize = 0x000;
pub(super) const MAC_PACKET_FILTER: usize = 0x008;
pub(super) const MAC_Q0_TX_FLOW_CTRL: usize = 0x070;
pub(super) const MAC_RX_FLOW_CTRL: usize = 0x090;
pub(super) const MAC_RXQ_CTRL0: usize = 0x0a0;
pub(super) const MAC_RXQ_CTRL1: usize = 0x0a4;
pub(super) const MAC_VERSION: usize = 0x110;
pub(super) const MAC_HW_FEATURE1: usize = 0x120;
pub(super) const MAC_MDIO_ADDRESS: usize = 0x200;
pub(super) const MAC_MDIO_DATA: usize = 0x204;
pub(super) const MAC_ADDRESS0_HIGH: usize = 0x300;
pub(super) const MAC_ADDRESS0_LOW: usize = 0x304;

pub(super) const MTL_TXQ0_OPERATION_MODE: usize = 0x0d00;
pub(super) const MTL_TXQ0_QUANTUM_WEIGHT: usize = 0x0d18;
pub(super) const MTL_RXQ0_OPERATION_MODE: usize = 0x0d30;

pub(super) const DMA_MODE: usize = 0x1000;
pub(super) const DMA_SYSBUS_MODE: usize = 0x1004;
pub(super) const DMA_CH0_CONTROL: usize = 0x1100;
pub(super) const DMA_CH0_TX_CONTROL: usize = 0x1104;
pub(super) const DMA_CH0_RX_CONTROL: usize = 0x1108;
pub(super) const DMA_CH0_TXDESC_LIST_HI: usize = 0x1110;
pub(super) const DMA_CH0_TXDESC_LIST: usize = 0x1114;
pub(super) const DMA_CH0_RXDESC_LIST_HI: usize = 0x1118;
pub(super) const DMA_CH0_RXDESC_LIST: usize = 0x111c;
pub(super) const DMA_CH0_TXDESC_TAIL: usize = 0x1120;
pub(super) const DMA_CH0_RXDESC_TAIL: usize = 0x1128;
pub(super) const DMA_CH0_TXDESC_RING_LEN: usize = 0x112c;
pub(super) const DMA_CH0_RXDESC_RING_LEN: usize = 0x1130;
pub(super) const DMA_CH0_INTERRUPT_ENABLE: usize = 0x1134;
pub(super) const DMA_CH0_STATUS: usize = 0x1160;

pub(super) const MAC_CONFIG_PS: u32 = 1 << 15;
pub(super) const MAC_CONFIG_FES: u32 = 1 << 14;
pub(super) const MAC_CONFIG_DM: u32 = 1 << 13;
pub(super) const MAC_CONFIG_TE: u32 = 1 << 1;
pub(super) const MAC_CONFIG_RE: u32 = 1;
pub(super) const MAC_PACKET_FILTER_PR: u32 = 1;

pub(super) const DMA_MODE_SWR: u32 = 1;
pub(super) const DMA_MODE_DCHE: u32 = 1 << 19;
pub(super) const DMA_SYSBUS_EAME: u32 = 1 << 11;
pub(super) const DMA_SYSBUS_BLEN16: u32 = 1 << 3;
pub(super) const DMA_SYSBUS_BLEN8: u32 = 1 << 2;
pub(super) const DMA_SYSBUS_BLEN4: u32 = 1 << 1;
pub(super) const DMA_CH_CONTROL_PBLX8: u32 = 1 << 16;
pub(super) const DMA_CH_CONTROL_DSL_64: u32 = 6 << 18;
pub(super) const DMA_TX_CONTROL_TXPBL_16: u32 = 16 << 16;
pub(super) const DMA_TX_CONTROL_OSP: u32 = 1 << 4;
pub(super) const DMA_TX_CONTROL_ST: u32 = 1;
pub(super) const DMA_RX_CONTROL_RXPBL_8: u32 = 8 << 16;
pub(super) const DMA_RX_CONTROL_SR: u32 = 1;

pub(super) const DMA_IRQ_TI: u32 = 1;
pub(super) const DMA_IRQ_RBU: u32 = 1 << 7;
pub(super) const DMA_IRQ_RI: u32 = 1 << 6;
pub(super) const DMA_IRQ_FBE: u32 = 1 << 12;
pub(super) const DMA_IRQ_AIE: u32 = 1 << 14;
pub(super) const DMA_IRQ_NIE: u32 = 1 << 15;
pub(super) const DMA_IRQ_MASK: u32 =
    DMA_IRQ_NIE | DMA_IRQ_AIE | DMA_IRQ_FBE | DMA_IRQ_RBU | DMA_IRQ_RI | DMA_IRQ_TI;

pub(super) const MDIO_BUSY: u32 = 1;
pub(super) const MDIO_READ: u32 = 3 << 2;
pub(super) const MDIO_CLOCK_250_300: u32 = 5 << 8;
pub(super) const PHY_BMSR: u8 = 1;
pub(super) const PHY_BMSR_LINK: u16 = 1 << 2;
