//! Pure helpers for firmware-owned portions of the board RAM map.

pub(crate) const fn top_reservation(
    range_base: usize,
    range_size: usize,
    reserved_size: usize,
) -> Option<(usize, usize)> {
    if reserved_size > range_size {
        return None;
    }
    Some((range_base + range_size - reserved_size, reserved_size))
}

#[cfg(test)]
mod tests {
    use super::top_reservation;

    #[test]
    fn reserves_the_top_16_mib_of_the_low_firmware_bank() {
        assert_eq!(
            top_reservation(0, 0x1000_0000, 0x0100_0000),
            Some((0x0f00_0000, 0x0100_0000))
        );
    }

    #[test]
    fn rejects_a_reservation_larger_than_its_bank() {
        assert_eq!(top_reservation(0x1000, 0x1000, 0x2000), None);
    }
}
