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
pub(crate) const DMA_STATUS_ERROR: u32 = DMA_STATUS_FBI | DMA_STATUS_AIS;
pub(crate) const DMA_STATUS_HANDLED: u32 =
    DMA_STATUS_RX | DMA_STATUS_TX | DMA_STATUS_ERROR | DMA_STATUS_NIS;
pub(crate) const DMA_IRQ_RX: u32 = DMA_STATUS_RX;
pub(crate) const DMA_IRQ_TX: u32 = DMA_STATUS_TX;
pub(crate) const DMA_IRQ_ERRORS: u32 = DMA_STATUS_ERROR;
pub(crate) const DMA_IRQ_DEFAULT: u32 = DMA_IRQ_RX | DMA_IRQ_TX | DMA_IRQ_ERRORS | DMA_STATUS_NIS;

pub(crate) fn masked_irq_enable(mut enable: u32, status: u32) -> u32 {
    if status & DMA_STATUS_RX != 0 {
        enable &= !DMA_IRQ_RX;
    }
    if status & DMA_STATUS_TX != 0 {
        enable &= !DMA_IRQ_TX;
    }
    if status & DMA_STATUS_ERROR != 0 {
        enable &= !(DMA_IRQ_RX | DMA_IRQ_TX | DMA_IRQ_ERRORS);
    }
    enable
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
