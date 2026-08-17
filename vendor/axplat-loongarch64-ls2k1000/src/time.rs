use core::sync::atomic::{AtomicU64, Ordering};

use axplat::time::TimeIf;
use lazyinit::LazyInit;
use loongArch64::time::Time;

static NANOS_PER_TICK: LazyInit<u64> = LazyInit::new();
const MIN_ONESHOT_TICKS: u64 = 4;

/// RTC wall-time offset from the architectural monotonic timer.
static RTC_EPOCHOFFSET_NANOS: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "rtc")]
const TOY_TRIM: usize = 0x20;
#[cfg(feature = "rtc")]
const TOY_WRITE0: usize = 0x24;
#[cfg(feature = "rtc")]
const TOY_WRITE1: usize = 0x28;
#[cfg(feature = "rtc")]
const TOY_READ0: usize = 0x2c;
#[cfg(feature = "rtc")]
const TOY_READ1: usize = 0x30;
#[cfg(feature = "rtc")]
const RTC_CTRL: usize = 0x40;
#[cfg(feature = "rtc")]
const TOY_ENABLE: u32 = 1 << 11;
#[cfg(feature = "rtc")]
const OSC_ENABLE: u32 = 1 << 8;

#[cfg(feature = "rtc")]
fn init_rtc() {
    use chrono::{TimeZone, Timelike, Utc};

    let Some(rtc_paddr) = crate::fdt::resources().rtc_paddr else {
        warn!("no enabled LS2K1000 RTC node in DTB; wall clock starts at epoch");
        return;
    };
    let base = crate::fdt::uncached_addr(rtc_paddr) as *mut u8;

    unsafe {
        // The hardware manual requires TOY_TRIM to be zero.  Preserve every
        // unrelated control bit already configured by firmware.
        (base.add(TOY_TRIM) as *mut u32).write_volatile(0);
        let ctrl = (base.add(RTC_CTRL) as *const u32).read_volatile();
        (base.add(RTC_CTRL) as *mut u32).write_volatile(ctrl | TOY_ENABLE | OSC_ENABLE);
    }

    // The year can change between the two MMIO reads at New Year.  Retry a
    // few times until both year samples agree; the low counter itself is a
    // self-consistent 32-bit snapshot with 100 ms resolution.
    let mut snapshot = None;
    for _ in 0..4 {
        let year_before = unsafe { (base.add(TOY_READ1) as *const u32).read_volatile() };
        let low = unsafe { (base.add(TOY_READ0) as *const u32).read_volatile() };
        let year_after = unsafe { (base.add(TOY_READ1) as *const u32).read_volatile() };
        if year_before == year_after {
            snapshot = Some((year_after, low));
            break;
        }
    }

    let Some((raw_year, low)) = snapshot else {
        warn!("LS2K1000 RTC did not provide a stable TOY snapshot");
        return;
    };

    fn bits(value: u32, shift: u32, width: u32) -> u32 {
        (value >> shift) & ((1 << width) - 1)
    }

    // Like Linux's struct rtc_time, the hardware year is years since 1900.
    // The upstream driver advertises the supported interval as 2000..=2099.
    let year = 1900_i32.saturating_add(raw_year as i32);
    let month = bits(low, 26, 6);
    let day = bits(low, 21, 5);
    let hour = bits(low, 16, 5);
    let minute = bits(low, 10, 6);
    let second = bits(low, 4, 6);
    let decisecond = bits(low, 0, 4);

    let date_time = (2000..=2099)
        .contains(&year)
        .then(|| {
            Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
                .single()
        })
        .flatten()
        .and_then(|time| time.with_nanosecond(decisecond * 100_000_000));

    let Some(epoch_nanos) = date_time.and_then(|time| time.timestamp_nanos_opt()) else {
        warn!(
            "LS2K1000 RTC contains an invalid TOY value: year={} low={:#010x}",
            raw_year, low
        );
        return;
    };

    let monotonic_nanos = TimeIfImpl::ticks_to_nanos(TimeIfImpl::current_ticks());
    RTC_EPOCHOFFSET_NANOS.store(
        (epoch_nanos as u64).saturating_sub(monotonic_nanos),
        Ordering::Relaxed,
    );
    info!(
        "LS2K1000 RTC initialized: {year:04}-{month:02}-{day:02} \
         {hour:02}:{minute:02}:{second:02} UTC"
    );
}

#[cfg(feature = "rtc")]
fn set_hardware_rtc(unix_seconds: u64) -> bool {
    use chrono::{Datelike, TimeZone, Timelike, Utc};

    let Ok(unix_seconds) = i64::try_from(unix_seconds) else {
        return false;
    };
    let Some(time) = Utc.timestamp_opt(unix_seconds, 0).single() else {
        return false;
    };
    let Some((low, high)) = crate::rtc_state::encode_toy(
        time.year(),
        time.month(),
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
        0,
    ) else {
        return false;
    };
    let Some(rtc_paddr) = crate::fdt::resources().rtc_paddr else {
        return false;
    };
    let base = crate::fdt::uncached_addr(rtc_paddr) as *mut u8;

    unsafe {
        // Match Linux's rtc-loongson set_time path: write SYS_TOYWRITE0 and
        // SYS_TOYWRITE1 in ascending register order, then ensure the TOY
        // counter and its 32.768 kHz crystal remain enabled.  The uncached DMW
        // mapping is strongly ordered, so the volatile stores reach hardware
        // in program order.
        (base.add(TOY_WRITE0) as *mut u32).write_volatile(low);
        (base.add(TOY_WRITE1) as *mut u32).write_volatile(high);
        let ctrl = (base.add(RTC_CTRL) as *const u32).read_volatile();
        (base.add(RTC_CTRL) as *mut u32).write_volatile(ctrl | TOY_ENABLE | OSC_ENABLE);
    }
    true
}

pub(super) fn init_early() {
    let frequency = loongArch64::time::get_timer_freq() as u64;
    NANOS_PER_TICK.init_once(axplat::time::NANOS_PER_SEC / frequency.max(1));

    #[cfg(feature = "rtc")]
    init_rtc();
}

pub(super) fn init_percpu() {
    #[cfg(feature = "irq")]
    {
        use loongArch64::register::tcfg;
        tcfg::set_init_val(MIN_ONESHOT_TICKS as _);
        tcfg::set_periodic(false);
        tcfg::set_en(true);
        axplat::irq::set_enable(crate::config::devices::TIMER_IRQ, true);
    }
}

struct TimeIfImpl;

#[impl_plat_interface]
impl TimeIf for TimeIfImpl {
    fn current_ticks() -> u64 {
        Time::read() as _
    }

    fn epochoffset_nanos() -> u64 {
        RTC_EPOCHOFFSET_NANOS.load(Ordering::Relaxed)
    }

    fn set_rtc_time(unix_seconds: u64) -> bool {
        #[cfg(feature = "rtc")]
        return set_hardware_rtc(unix_seconds);
        #[cfg(not(feature = "rtc"))]
        false
    }

    fn ticks_to_nanos(ticks: u64) -> u64 {
        ticks.saturating_mul(*NANOS_PER_TICK)
    }

    fn nanos_to_ticks(nanos: u64) -> u64 {
        nanos / *NANOS_PER_TICK
    }

    #[cfg(feature = "irq")]
    fn irq_num() -> usize {
        crate::config::devices::TIMER_IRQ
    }

    #[cfg(feature = "irq")]
    fn set_oneshot_timer(deadline_ns: u64) {
        use loongArch64::register::{tcfg, ticlr};

        let now = Self::current_ticks();
        let deadline = Self::nanos_to_ticks(deadline_ns);
        let delta = deadline
            .saturating_sub(now)
            .max(MIN_ONESHOT_TICKS)
            .next_multiple_of(MIN_ONESHOT_TICKS);

        tcfg::set_en(false);
        ticlr::clear_timer_interrupt();
        tcfg::set_periodic(false);
        tcfg::set_init_val(delta as _);
        tcfg::set_en(true);
    }
}
