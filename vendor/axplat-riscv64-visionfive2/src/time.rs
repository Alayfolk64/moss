use axplat::time::{NANOS_PER_SEC, TimeIf};
use riscv::register::time;

const NANOS_PER_TICK: u64 = NANOS_PER_SEC / crate::config::devices::TIMER_FREQUENCY as u64;

pub(super) fn init_early() {
    // U-Boot may leave a supervisor timer event pending. Keep it disarmed until
    // axruntime has installed its handler and explicitly programs the first tick.
    sbi_rt::set_timer(u64::MAX);
}

pub(super) fn init_percpu() {
    // Each hart owns an independent supervisor timer comparator. Trigger one
    // initial interrupt so axruntime can establish its per-CPU tick deadline.
    #[cfg(feature = "irq")]
    sbi_rt::set_timer(0);
}

struct TimeIfImpl;

#[impl_plat_interface]
impl TimeIf for TimeIfImpl {
    fn current_ticks() -> u64 {
        time::read() as u64
    }

    fn ticks_to_nanos(ticks: u64) -> u64 {
        ticks * NANOS_PER_TICK
    }

    fn nanos_to_ticks(nanos: u64) -> u64 {
        nanos / NANOS_PER_TICK
    }

    fn epochoffset_nanos() -> u64 {
        0
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> usize {
        crate::config::devices::TIMER_IRQ
    }

    #[cfg(feature = "irq")]
    fn set_oneshot_timer(deadline_ns: u64) {
        sbi_rt::set_timer(Self::nanos_to_ticks(deadline_ns));
    }
}
