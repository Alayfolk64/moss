//! StarFive JH7110 watchdog used by unattended VisionFive 2 runs.

use core::{
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use fdt_parser::{Fdt, Status};

use crate::config::{devices, plat::PHYS_VIRT_OFFSET};

const LOAD: usize = 0x000;
const VALUE: usize = 0x004;
const CONTROL: usize = 0x008;
const INT_CLEAR: usize = 0x00c;
const INTERRUPT_STATUS: usize = 0x014;
const LOCK: usize = 0xc00;

const CONTROL_ENABLE: u32 = 1 << 0;
const CONTROL_RESET_ENABLE: u32 = 1 << 1;
const INT_CLEAR_RELOAD: u32 = 1;
const UNLOCK_KEY: u32 = 0x1acce551;

const CORE_CLOCK_HZ: u64 = 24_000_000;
const CLOCK_ENABLE: u32 = 1 << 31;
const CLOCKS: [usize; 2] = [122, 123];
const RESETS: [usize; 2] = [109, 110];
const RESET_ASSERT: usize = 0x2f8;
const RESET_STATUS: usize = 0x308;
const RESET_TIMEOUT: usize = 100_000;

const EARLY_BOOT_TIMEOUT_SECS: u32 = 180;

static WATCHDOG_PADDR: AtomicUsize = AtomicUsize::new(0);
static INIT_STATUS: AtomicUsize = AtomicUsize::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);

const INIT_DTB_REJECTED: usize = 1;
const INIT_CLOCK_RESET_FAILED: usize = 2;
const INIT_READY: usize = 3;

#[inline]
fn read(base: usize, offset: usize) -> u32 {
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

#[inline]
fn write(base: usize, offset: usize, value: u32) {
    unsafe { ((base + offset) as *mut u32).write_volatile(value) };
    unsafe { core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags)) };
}

fn property_pairs_match(node: &fdt_parser::Node<'_>, name: &str, expected: &[u32]) -> bool {
    let Some(property) = node.find_property(name) else {
        return false;
    };
    let mut values = property.u32_list();
    for expected_id in expected {
        if values.next().is_none() || values.next() != Some(*expected_id) {
            return false;
        }
    }
    values.next().is_none()
}

fn configured_paddr(dtb_paddr: usize) -> Option<usize> {
    let ptr = NonNull::new((dtb_paddr + PHYS_VIRT_OFFSET) as *mut u8)?;
    let fdt = Fdt::from_ptr(ptr).ok()?;
    // OpenSBI passes U-Boot's compact JH7110 DTB at 0x4220_0000.  That tree
    // identifies the SoC but omits the watchdog node that is present in the
    // Linux DTB.  This platform crate has a fixed, reserved JH7110 MMIO map,
    // so the SoC compatible is the required contract; validate the detailed
    // node too whenever firmware supplies it.
    fdt.find_compatible(&["starfive,jh7110"]).next()?;
    let node = fdt
        .find_compatible(&["starfive,jh7110-wdt"])
        .find(|node| node.status() != Some(Status::Disabled));
    let Some(node) = node else {
        return Some(devices::WATCHDOG_PADDR as usize);
    };
    let reg = node.reg()?.next()?;
    let paddr = usize::try_from(reg.address).ok()?;
    let size = usize::try_from(reg.size?).ok()?;
    if paddr != devices::WATCHDOG_PADDR as usize || size < LOCK + size_of::<u32>() {
        return None;
    }
    if !property_pairs_match(&node, "clocks", &[122, 123])
        || !property_pairs_match(&node, "resets", &[109, 110])
    {
        return None;
    }
    Some(paddr)
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

fn register_base() -> Option<usize> {
    let paddr = WATCHDOG_PADDR.load(Ordering::Acquire);
    (paddr != 0).then_some(paddr + PHYS_VIRT_OFFSET)
}

fn timeout_count(timeout_secs: u32) -> Option<u32> {
    CORE_CLOCK_HZ
        .checked_mul(u64::from(timeout_secs))
        .map(|count| count / 2)
        .and_then(|count| u32::try_from(count).ok())
        .filter(|count| *count != 0)
}

fn unlock(base: usize) {
    write(base, LOCK, UNLOCK_KEY);
}

fn lock(base: usize) {
    write(base, LOCK, !UNLOCK_KEY);
}

pub(crate) fn init_early(dtb_paddr: usize) {
    let Some(paddr) = configured_paddr(dtb_paddr) else {
        INIT_STATUS.store(INIT_DTB_REJECTED, Ordering::Release);
        warn!("VF2 DTB has no usable JH7110 watchdog contract");
        return;
    };
    WATCHDOG_PADDR.store(paddr, Ordering::Release);
    if !enable_clock_reset() {
        INIT_STATUS.store(INIT_CLOCK_RESET_FAILED, Ordering::Release);
        warn!("VF2 watchdog clock/reset initialization timed out");
        WATCHDOG_PADDR.store(0, Ordering::Release);
        return;
    }
    INIT_STATUS.store(INIT_READY, Ordering::Release);
    if start(EARLY_BOOT_TIMEOUT_SECS) {
        info!(
            "VF2 early watchdog armed: timeout={} s, clock={} Hz",
            EARLY_BOOT_TIMEOUT_SECS, CORE_CLOCK_HZ
        );
    }
}

pub(crate) fn start(timeout_secs: u32) -> bool {
    let Some(base) = register_base() else {
        error!(
            "VF2 watchdog unavailable: early init status={}",
            INIT_STATUS.load(Ordering::Acquire)
        );
        return false;
    };
    let Some(count) = timeout_count(timeout_secs) else {
        error!(
            "VF2 watchdog timeout {} s is outside the 1..=357 s hardware range",
            timeout_secs
        );
        return false;
    };

    unlock(base);
    let control = read(base, CONTROL) & !CONTROL_ENABLE;
    write(base, CONTROL, control);
    write(base, CONTROL, control | CONTROL_RESET_ENABLE);
    write(base, INT_CLEAR, INT_CLEAR_RELOAD);
    write(base, LOAD, count);
    // MOSS can inherit an already-running lease installed by U-Boot.  The
    // first clear above acknowledges either stage of that lease; reload once
    // more after changing LOAD so VALUE immediately adopts the new timeout.
    write(base, INT_CLEAR, INT_CLEAR_RELOAD);
    write(
        base,
        CONTROL,
        control | CONTROL_RESET_ENABLE | CONTROL_ENABLE,
    );
    lock(base);
    let programmed_load = read(base, LOAD);
    let programmed_control = read(base, CONTROL);
    if programmed_load != count
        || programmed_control & (CONTROL_ENABLE | CONTROL_RESET_ENABLE)
            != CONTROL_ENABLE | CONTROL_RESET_ENABLE
    {
        error!(
            "VF2 watchdog readback failed: load={:#x}/{:#x}, control={:#x}",
            programmed_load, count, programmed_control
        );
        ARMED.store(false, Ordering::Release);
        return false;
    }
    ARMED.store(true, Ordering::Release);
    info!(
        "VF2 watchdog armed: timeout={} s, clock={} Hz, stage_count={}, value={}, ims={:#x}",
        timeout_secs,
        CORE_CLOCK_HZ,
        count,
        read(base, VALUE),
        read(base, INTERRUPT_STATUS)
    );
    true
}

pub(crate) fn ping() {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    let Some(base) = register_base() else {
        return;
    };
    unlock(base);
    write(base, INT_CLEAR, INT_CLEAR_RELOAD);
    lock(base);
}

pub(crate) fn stop() {
    ARMED.store(false, Ordering::Release);
    let Some(base) = register_base() else {
        return;
    };
    unlock(base);
    write(base, CONTROL, read(base, CONTROL) & !CONTROL_ENABLE);
    lock(base);
}
