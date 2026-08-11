//! Pure LS2K1000 node-PLL field encoding and frequency calculation.

pub(crate) const TARGET_MHZ: u64 = 900;
pub(crate) const TARGET_LOW_CONFIG: u64 = (72 << 32) | (1 << 42) | (4 << 26) | (3 << 10) | (1 << 7);
pub(crate) const TARGET_HIGH_CONFIG: u64 = 2;
pub(crate) const FULL_FREQSCALE: u64 = 7;

const REF_CLOCK_MHZ: u64 = 100;

/// Return the effective node clock after the NODE_FREQSCALE multiplier.
pub(crate) const fn effective_mhz(low: u64, high: u64, freqscale: u64) -> Option<u64> {
    let loopc = (low >> 32) & 0x3ff;
    let div = (low >> 42) & 0x3f;
    let refc = (low >> 26) & 0x3f;
    let div_l2 = high & 0x3f;
    let scale_numerator = (freqscale & 0x7) + 1;
    let denominator = refc * div * div_l2 * 8;
    if loopc == 0 || denominator == 0 {
        return None;
    }
    Some(REF_CLOCK_MHZ * loopc * scale_numerator / denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_900_mhz_configuration_is_exact() {
        assert_eq!(TARGET_LOW_CONFIG, 0x0000_0448_1000_0c80);
        assert_eq!(
            effective_mhz(TARGET_LOW_CONFIG, TARGET_HIGH_CONFIG, 7),
            Some(900)
        );
    }

    #[test]
    fn freqscale_is_part_of_the_effective_clock() {
        assert_eq!(
            effective_mhz(TARGET_LOW_CONFIG, TARGET_HIGH_CONFIG, 3),
            Some(450)
        );
    }

    #[test]
    fn invalid_divisor_is_rejected() {
        assert_eq!(effective_mhz(TARGET_LOW_CONFIG, 0, 7), None);
    }
}
