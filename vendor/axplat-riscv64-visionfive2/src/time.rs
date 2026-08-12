use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axplat::time::{NANOS_PER_SEC, TimeIf};
use riscv::register::time;

const NANOS_PER_TICK: u64 = NANOS_PER_SEC / crate::config::devices::TIMER_FREQUENCY as u64;
static RTC_EPOCHOFFSET_NANOS: AtomicU64 = AtomicU64::new(0);
static RTC_READY: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "rtc")]
mod rtc {
    use core::{num::NonZeroUsize, ptr::NonNull};

    use chrono::{Datelike, TimeZone, Timelike, Utc};
    use fdt_parser::{Fdt, Status};

    use super::{NANOS_PER_SEC, RTC_EPOCHOFFSET_NANOS, RTC_READY, TimeIf, TimeIfImpl};
    use crate::config::{devices, plat::PHYS_VIRT_OFFSET};

    const RTC_CFG: usize = 0x00;
    const RTC_IRQ_EVENT: usize = 0x14;
    const RTC_IRQ_STATUS: usize = 0x18;
    const RTC_CFG_TIME: usize = 0x28;
    const RTC_CFG_DATE: usize = 0x2c;
    const RTC_TIME: usize = 0x3c;
    const RTC_DATE: usize = 0x40;
    const RTC_ENABLE: u32 = 1;
    const RTC_24_HOUR: u32 = 1 << 3;
    const RTC_ONE_SECOND: u32 = 1 << 3;
    const RTC_UPDATE: u32 = 1 << 31;

    const CLOCK_ENABLE: u32 = 1 << 31;
    const CLOCKS: [usize; 3] = [229, 231, 232];
    const RESETS: [usize; 3] = [165, 166, 167];
    const RESET_ASSERT: usize = 0x2f8;
    const RESET_STATUS: usize = 0x308;
    const RESET_TIMEOUT: usize = 100_000;
    const RTC_WRITE_TIMEOUT_MS: u64 = 1_200;

    fn read(base: usize, offset: usize) -> u32 {
        unsafe { ((base + offset) as *const u32).read_volatile() }
    }

    fn write(base: usize, offset: usize, value: u32) {
        unsafe { ((base + offset) as *mut u32).write_volatile(value) }
    }

    fn bcd(value: u32) -> Option<u32> {
        let low = value & 0xf;
        let high = value >> 4;
        (low < 10 && high < 10).then_some(high * 10 + low)
    }

    fn to_bcd(value: u32) -> u32 {
        (value / 10) << 4 | value % 10
    }

    fn field(value: u32, shift: u32, width: u32) -> u32 {
        (value >> shift) & ((1 << width) - 1)
    }

    fn parse_datetime(time: u32, date: u32) -> Option<u64> {
        let second = bcd(field(time, 0, 7))?;
        let minute = bcd(field(time, 7, 7))?;
        let hour = bcd(field(time, 14, 7))?;
        let day = bcd(field(date, 0, 6))?;
        let month = bcd(field(date, 6, 5))?;
        let year = 2000_i32.checked_add(bcd(field(date, 11, 8))? as i32)?;
        Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
            .single()
            .and_then(|value| value.timestamp().try_into().ok())
    }

    fn encode_datetime(unix_seconds: u64) -> Option<(u32, u32)> {
        let value = Utc
            .timestamp_opt(unix_seconds.try_into().ok()?, 0)
            .single()?;
        if !(2001..=2099).contains(&value.year()) {
            return None;
        }
        let time =
            to_bcd(value.second()) | to_bcd(value.minute()) << 7 | to_bcd(value.hour()) << 14;
        let date = to_bcd(value.day())
            | to_bcd(value.month()) << 6
            | to_bcd((value.year() - 2000) as u32) << 11;
        Some((time, date))
    }

    fn configured_rtc_paddr(dtb_paddr: usize) -> Option<usize> {
        let ptr = NonNull::new((dtb_paddr + PHYS_VIRT_OFFSET) as *mut u8)?;
        let fdt = Fdt::from_ptr(ptr).ok()?;
        let node = fdt
            .find_compatible(&["starfive,jh7110-rtc"])
            .find(|node| node.status() != Some(Status::Disabled))?;
        let reg = node.reg()?.next()?;
        let paddr = usize::try_from(reg.address).ok()?;
        (paddr == devices::RTC_PADDR as usize).then_some(paddr)
    }

    fn enable_clock_reset() -> bool {
        let base = devices::SYSCRG_PADDR as usize + PHYS_VIRT_OFFSET;
        for clock in CLOCKS {
            let offset = clock * size_of::<u32>();
            write(base, offset, read(base, offset) | CLOCK_ENABLE);
        }

        for reset in RESETS {
            let word = reset / u32::BITS as usize;
            let mask = 1 << (reset % u32::BITS as usize);
            let offset = RESET_ASSERT + word * size_of::<u32>();
            write(base, offset, read(base, offset) & !mask);
        }

        for _ in 0..RESET_TIMEOUT {
            let ready = RESETS.iter().all(|reset| {
                let word = reset / u32::BITS as usize;
                let mask = 1 << (reset % u32::BITS as usize);
                read(base, RESET_STATUS + word * size_of::<u32>()) & mask != 0
            });
            if ready {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn read_time(base: usize) -> Option<u64> {
        if read(base, RTC_CFG) & RTC_ENABLE == 0 {
            return None;
        }
        for _ in 0..4 {
            let second_state = read(base, RTC_IRQ_STATUS) & RTC_ONE_SECOND;
            let time = read(base, RTC_TIME);
            let date = read(base, RTC_DATE);
            if read(base, RTC_IRQ_STATUS) & RTC_ONE_SECOND == second_state {
                return parse_datetime(time, date);
            }
        }
        None
    }

    fn write_time(base: usize, unix_seconds: u64) -> bool {
        let Some((time, date)) = encode_datetime(unix_seconds) else {
            return false;
        };
        write(base, RTC_CFG_TIME, time);
        write(base, RTC_CFG_DATE, date);
        write(base, RTC_IRQ_EVENT, read(base, RTC_IRQ_EVENT) | RTC_UPDATE);
        unsafe { core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags)) };

        let deadline = TimeIfImpl::ticks_to_nanos(TimeIfImpl::current_ticks())
            .saturating_add(RTC_WRITE_TIMEOUT_MS * 1_000_000);
        while TimeIfImpl::ticks_to_nanos(TimeIfImpl::current_ticks()) < deadline {
            if read_time(base).is_some_and(|actual| actual.abs_diff(unix_seconds) <= 1) {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn build_epoch() -> u64 {
        env!("MOSS_BUILD_EPOCH")
            .parse()
            .expect("build script supplied an invalid MOSS_BUILD_EPOCH")
    }

    pub(super) fn init(dtb_paddr: usize) {
        let Some(paddr) = configured_rtc_paddr(dtb_paddr) else {
            warn!("VF2 DTB has no enabled JH7110 RTC; using image build time");
            set_epoch_offset(build_epoch());
            return;
        };
        if !enable_clock_reset() {
            warn!("VF2 RTC clock/reset initialization timed out; using image build time");
            set_epoch_offset(build_epoch());
            return;
        }

        let base = paddr + PHYS_VIRT_OFFSET;
        write(
            base,
            RTC_CFG,
            read(base, RTC_CFG) | RTC_ENABLE | RTC_24_HOUR,
        );
        RTC_READY.store(true, super::Ordering::Release);
        let rtc_epoch = read_time(base).unwrap_or(0);
        let epoch = rtc_epoch.max(build_epoch());
        set_epoch_offset(epoch);
        if rtc_epoch < epoch && !write_time(base, epoch) {
            warn!("VF2 RTC rejected build-time fallback");
        }
        info!("VF2 RTC initialized at Unix time {epoch}");
    }

    fn set_epoch_offset(epoch_seconds: u64) {
        let monotonic = TimeIfImpl::ticks_to_nanos(TimeIfImpl::current_ticks());
        RTC_EPOCHOFFSET_NANOS.store(
            epoch_seconds
                .saturating_mul(NANOS_PER_SEC)
                .saturating_sub(monotonic),
            super::Ordering::Relaxed,
        );
    }

    pub(super) fn set_time(unix_seconds: u64) -> bool {
        if !RTC_READY.load(super::Ordering::Acquire) {
            return false;
        }
        let base = devices::RTC_PADDR as usize + PHYS_VIRT_OFFSET;
        NonZeroUsize::new(base).is_some() && write_time(base, unix_seconds)
    }

    #[cfg(test)]
    mod tests {
        use super::{encode_datetime, parse_datetime};

        #[test]
        fn rtc_round_trip() {
            let epoch = 1_786_502_345;
            let (time, date) = encode_datetime(epoch).unwrap();
            assert_eq!(parse_datetime(time, date), Some(epoch));
        }

        #[test]
        fn rtc_rejects_invalid_bcd_and_range() {
            assert_eq!(parse_datetime(0x7a, 0), None);
            assert_eq!(encode_datetime(0), None);
        }
    }
}

pub(super) fn init_early(dtb_paddr: usize) {
    // U-Boot may leave a supervisor timer event pending. Keep it disarmed until
    // axruntime has installed its handler and explicitly programs the first tick.
    sbi_rt::set_timer(u64::MAX);
    #[cfg(feature = "rtc")]
    rtc::init(dtb_paddr);
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
        RTC_EPOCHOFFSET_NANOS.load(Ordering::Relaxed)
    }

    fn set_rtc_time(unix_seconds: u64) -> bool {
        #[cfg(feature = "rtc")]
        return rtc::set_time(unix_seconds);
        #[cfg(not(feature = "rtc"))]
        false
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
