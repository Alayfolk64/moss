//! Time-related operations.

use core::sync::atomic::{AtomicI64, Ordering};
pub use core::time::Duration;

/// A measurement of the system clock.
///
/// Currently, it reuses the [`core::time::Duration`] type. But it does not
/// represent a duration, but a clock time.
pub type TimeValue = Duration;

/// Number of milliseconds in a second.
pub const MILLIS_PER_SEC: u64 = 1_000;
/// Number of microseconds in a second.
pub const MICROS_PER_SEC: u64 = 1_000_000;
/// Number of nanoseconds in a second.
pub const NANOS_PER_SEC: u64 = 1_000_000_000;
/// Number of nanoseconds in a millisecond.
pub const NANOS_PER_MILLIS: u64 = 1_000_000;
/// Number of nanoseconds in a microsecond.
pub const NANOS_PER_MICROS: u64 = 1_000;

static REALTIME_OFFSET_NANOS: AtomicI64 = AtomicI64::new(0);

/// Time-related interfaces.
#[def_plat_interface]
pub trait TimeIf {
    /// Returns the current clock time in hardware ticks.
    fn current_ticks() -> u64;

    /// Converts hardware ticks to nanoseconds.
    fn ticks_to_nanos(ticks: u64) -> u64;

    /// Converts nanoseconds to hardware ticks.
    fn nanos_to_ticks(nanos: u64) -> u64;

    /// Return epoch offset in nanoseconds (wall time offset to monotonic
    /// clock start).
    fn epochoffset_nanos() -> u64;

    /// Persists a UTC Unix timestamp to the platform RTC, if available.
    fn set_rtc_time(_unix_seconds: u64) -> bool {
        false
    }

    /// Returns the IRQ number for the timer interrupt.
    #[cfg(feature = "irq")]
    fn irq_num() -> usize;

    /// Set a one-shot timer.
    ///
    /// A timer interrupt will be triggered at the specified monotonic time
    /// deadline (in nanoseconds).
    #[cfg(feature = "irq")]
    fn set_oneshot_timer(deadline_ns: u64);
}

/// Returns nanoseconds elapsed since system boot.
pub fn monotonic_time_nanos() -> u64 {
    ticks_to_nanos(current_ticks())
}

/// Returns the time elapsed since system boot in [`TimeValue`].
pub fn monotonic_time() -> TimeValue {
    TimeValue::from_nanos(monotonic_time_nanos())
}

/// Returns nanoseconds elapsed since epoch (also known as realtime).
pub fn wall_time_nanos() -> u64 {
    monotonic_time_nanos() + epochoffset_nanos()
}

/// Returns the time elapsed since epoch (also known as realtime) in [`TimeValue`].
pub fn wall_time() -> TimeValue {
    TimeValue::from_nanos(monotonic_time_nanos() + epochoffset_nanos())
}

/// Sets the offset applied to realtime clock reads.
pub fn set_realtime_offset_nanos(offset: i64) {
    REALTIME_OFFSET_NANOS.store(offset, Ordering::Relaxed);
}

/// Returns the offset applied to realtime clock reads.
pub fn realtime_offset_nanos() -> i64 {
    REALTIME_OFFSET_NANOS.load(Ordering::Relaxed)
}

fn add_realtime_offset(nanos: u64, offset: i64) -> u64 {
    if offset >= 0 {
        nanos.saturating_add(offset as u64)
    } else {
        nanos.saturating_sub(offset.unsigned_abs())
    }
}

/// Returns adjusted realtime nanoseconds elapsed since epoch.
pub fn realtime_time_nanos() -> u64 {
    add_realtime_offset(wall_time_nanos(), realtime_offset_nanos())
}

/// Returns adjusted realtime elapsed since epoch.
pub fn realtime_time() -> TimeValue {
    TimeValue::from_nanos(realtime_time_nanos())
}

/// Busy waiting for the given duration.
pub fn busy_wait(dur: Duration) {
    busy_wait_until(wall_time() + dur);
}

/// Busy waiting until reaching the given deadline.
pub fn busy_wait_until(deadline: TimeValue) {
    while wall_time() < deadline {
        core::hint::spin_loop();
    }
}
