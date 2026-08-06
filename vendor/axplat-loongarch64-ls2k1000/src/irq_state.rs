//! Pure helpers for the LS2K1000 interrupt dispatch policy.

pub(crate) fn first_pending_source(pending: u32) -> Option<usize> {
    (pending != 0).then_some(pending.trailing_zeros() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_exactly_one_pending_source() {
        assert_eq!(first_pending_source(0), None);
        assert_eq!(first_pending_source(1 << 7), Some(7));
        assert_eq!(first_pending_source((1 << 19) | (1 << 12)), Some(12));
    }
}
