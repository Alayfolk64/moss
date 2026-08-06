//! Overflow-safe block range validation shared by in-memory drivers.

use core::ops::Range;

pub(crate) fn checked_block_range(
    block_id: u64,
    byte_len: usize,
    block_size: usize,
    capacity: usize,
) -> Option<Range<usize>> {
    if block_size == 0 || byte_len % block_size != 0 {
        return None;
    }
    let block = usize::try_from(block_id).ok()?;
    let start = block.checked_mul(block_size)?;
    let end = start.checked_add(byte_len)?;
    (end <= capacity).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_aligned_ranges_inside_capacity() {
        assert_eq!(checked_block_range(0, 512, 512, 1024), Some(0..512));
        assert_eq!(checked_block_range(1, 512, 512, 1024), Some(512..1024));
        assert_eq!(checked_block_range(2, 0, 512, 1024), Some(1024..1024));
    }

    #[test]
    fn rejects_unaligned_out_of_bounds_and_overflowing_ranges() {
        assert_eq!(checked_block_range(0, 513, 512, 1024), None);
        assert_eq!(checked_block_range(1, 1024, 512, 1024), None);
        assert_eq!(checked_block_range(u64::MAX, 512, 512, usize::MAX), None);
    }
}
