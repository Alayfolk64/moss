//! Pure LS2K1000 device-tree interrupt translation helpers.

pub(crate) const LIOINTC_IRQ_BASE: usize = 16;
pub(crate) const LIOINTC_IRQS_PER_CONTROLLER: usize = 32;

pub(crate) const fn translated_liointc_irq(
    controller_index: usize,
    source: usize,
) -> Option<usize> {
    if source >= LIOINTC_IRQS_PER_CONTROLLER {
        return None;
    }
    let Some(controller_offset) = controller_index.checked_mul(LIOINTC_IRQS_PER_CONTROLLER) else {
        return None;
    };
    let Some(controller_base) = LIOINTC_IRQ_BASE.checked_add(controller_offset) else {
        return None;
    };
    controller_base.checked_add(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_each_controller_into_a_disjoint_irq_range() {
        assert_eq!(translated_liointc_irq(0, 12), Some(28));
        assert_eq!(translated_liointc_irq(1, 19), Some(67));
    }

    #[test]
    fn rejects_sources_outside_a_liointc_bank() {
        assert_eq!(translated_liointc_irq(0, 31), Some(47));
        assert_eq!(translated_liointc_irq(0, 32), None);
        assert_eq!(translated_liointc_irq(usize::MAX, 0), None);
    }
}
