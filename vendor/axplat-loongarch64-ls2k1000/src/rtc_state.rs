//! Pure LS2K1000 TOY register encoding helpers.

const YEAR_BASE: i32 = 1900;

/// Encode a UTC calendar value for `SYS_TOYWRITE0/1`.
///
/// The LS2K1000 TOY counter supports years 2000 through 2099.  The high
/// register stores years since 1900, matching Linux's `struct rtc_time`, and
/// the low register stores month/day/time plus one decimal-second digit.
pub(crate) const fn encode_toy(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    decisecond: u32,
) -> Option<(u32, u32)> {
    if year < 2000
        || year > 2099
        || month < 1
        || month > 12
        || day < 1
        || day > 31
        || hour > 23
        || minute > 59
        || second > 59
        || decisecond > 9
    {
        return None;
    }

    let low =
        (month << 26) | (day << 21) | (hour << 16) | (minute << 10) | (second << 4) | decisecond;
    Some((low, (year - YEAR_BASE) as u32))
}

#[cfg(test)]
mod tests {
    use super::encode_toy;

    #[test]
    fn encodes_linux_compatible_toy_fields() {
        assert_eq!(
            encode_toy(2026, 8, 17, 10, 35, 20, 0),
            Some((0x222a_8d40, 126))
        );
    }

    #[test]
    fn accepts_hardware_year_boundaries() {
        assert!(encode_toy(2000, 1, 1, 0, 0, 0, 0).is_some());
        assert!(encode_toy(2099, 12, 31, 23, 59, 59, 9).is_some());
    }

    #[test]
    fn rejects_out_of_range_fields() {
        assert_eq!(encode_toy(1999, 1, 1, 0, 0, 0, 0), None);
        assert_eq!(encode_toy(2100, 1, 1, 0, 0, 0, 0), None);
        assert_eq!(encode_toy(2026, 13, 1, 0, 0, 0, 0), None);
        assert_eq!(encode_toy(2026, 1, 1, 24, 0, 0, 0), None);
        assert_eq!(encode_toy(2026, 1, 1, 0, 60, 0, 0), None);
        assert_eq!(encode_toy(2026, 1, 1, 0, 0, 60, 0), None);
    }
}
