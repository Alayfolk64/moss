//! Loongson 2K1000 watchdog driven entirely from firmware DT resources.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::fdt::{WatchdogResource, uncached_addr};

const RST_CTR: usize = 0x00;
#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
const WD_CR: usize = 0x04;
#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
const WD_TIMER: usize = 0x08;
#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
const RST_CTR_ENABLE: u32 = 1 << 1;
#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
const WDT_CLOCK_HZ: u64 = 125_000_000;

#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
pub(crate) const BOOT_TIMEOUT_SECS: u32 = 30;

static ARMED: AtomicBool = AtomicBool::new(false);

fn resource() -> Option<WatchdogResource> {
    crate::fdt::resources().watchdog
}

#[inline]
fn register(resource: WatchdogResource, offset: usize) -> *mut u32 {
    debug_assert!(offset + core::mem::size_of::<u32>() <= resource.size);
    (uncached_addr(resource.paddr) + offset) as *mut u32
}

#[inline]
unsafe fn write(resource: WatchdogResource, offset: usize, value: u32) {
    unsafe { register(resource, offset).write_volatile(value) };
    unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };
}

#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
fn timeout_count(timeout_secs: u32) -> Option<u32> {
    WDT_CLOCK_HZ
        .checked_mul(u64::from(timeout_secs))
        .and_then(|count| u32::try_from(count).ok())
        .filter(|count| *count != 0)
}

#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
pub(crate) fn start(timeout_secs: u32) -> bool {
    let Some(resource) = resource() else {
        return false;
    };
    let Some(count) = timeout_count(timeout_secs) else {
        error!(
            "LS2K1000 watchdog timeout {} s is outside the 1..=34 s hardware range",
            timeout_secs
        );
        return false;
    };

    unsafe {
        // Match the vendor Linux driver: stop, program the APB-clock count,
        // enable reset mode, then reload the down-counter.
        write(resource, RST_CTR, 0);
        write(resource, WD_TIMER, count);
        write(resource, RST_CTR, RST_CTR_ENABLE);
        write(resource, WD_CR, 1);
    }
    ARMED.store(true, Ordering::Release);
    info!(
        "LS2K1000 watchdog armed: timeout={} s, clock={} Hz, count={}",
        timeout_secs, WDT_CLOCK_HZ, count
    );
    true
}

#[cfg(all(feature = "watchdog", not(feature = "shell-mode")))]
pub(crate) fn ping() {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    if let Some(resource) = resource() {
        unsafe { write(resource, WD_CR, 1) };
    }
}

pub(crate) fn stop() {
    ARMED.store(false, Ordering::Release);
    if let Some(resource) = resource() {
        unsafe { write(resource, RST_CTR, 0) };
    }
}
