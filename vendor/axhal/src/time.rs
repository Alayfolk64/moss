//! Time-related operations.

// `irq_num` exists only in the git (unpublished) version of axplat, not in 0.3.0.
#[cfg(all(feature = "irq", any(feature = "defplat", feature = "myplat")))]
pub use axplat::time::irq_num;
#[cfg(feature = "irq")]
pub use axplat::time::set_oneshot_timer;
pub use axplat::time::{
    Duration, MICROS_PER_SEC, MILLIS_PER_SEC, NANOS_PER_MICROS, NANOS_PER_MILLIS, NANOS_PER_SEC,
    TimeValue, busy_wait, busy_wait_until, current_ticks, epochoffset_nanos, monotonic_time,
    monotonic_time_nanos, nanos_to_ticks, realtime_offset_nanos, realtime_time,
    realtime_time_nanos, set_realtime_offset_nanos, set_rtc_time, ticks_to_nanos, wall_time,
    wall_time_nanos,
};
#[cfg(all(feature = "irq", not(any(feature = "defplat", feature = "myplat"))))]
pub fn irq_num() -> usize {
    0
}
