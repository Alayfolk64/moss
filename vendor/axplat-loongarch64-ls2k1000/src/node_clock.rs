//! Temporary node-clock setup for board experiments.
//!
//! This follows the runtime sequence in Loongson's Linux 5.10
//! `clk-loongson2k1000.c`.  Unlike the early U-Boot sequence, runtime code must
//! first switch away from the live PLL in a separate store before powering it
//! down. It changes only the node PLL (CPU/cache/node fabric), never SPI flash,
//! and a board reset restores the firmware configuration.

use crate::fdt::uncached_addr;
use crate::node_clock_state::{
    FULL_FREQSCALE, TARGET_HIGH_CONFIG, TARGET_LOW_CONFIG, TARGET_MHZ, effective_mhz,
};

const NODE_PLL_LOW_PADDR: usize = 0x1fe0_0480;
const NODE_PLL_HIGH_PADDR: usize = 0x1fe0_0488;
const NODE_FREQSCALE_PADDR: usize = 0x1fe0_04d0;

const PLL_POWER_DOWN: u64 = 1 << 19;
const PLL_LOCKED: u64 = 1 << 16;
const PLL_SOFT_SET: u64 = 1 << 2;
const PLL_SELECT: u64 = 1;
const LOCK_POLL_LIMIT: usize = 1_000_000;

#[inline]
unsafe fn read64(paddr: usize) -> u64 {
    unsafe { (uncached_addr(paddr) as *const u64).read_volatile() }
}

#[inline]
unsafe fn write64(paddr: usize, value: u64) {
    // These registers are reached through the strongly ordered uncached DMW.
    // Do not insert a DBAR between power-down and the following programming
    // stores: Loongson's clksetting.S deliberately emits consecutive st.d
    // instructions while the node clock is on the 100 MHz reference path.
    unsafe { (uncached_addr(paddr) as *mut u64).write_volatile(value) };
}

/// Switch the boot CPU from the factory node clock to Loongson's official
/// 900 MHz/1.05 V U-Boot setting. This runs before any secondary CPU starts.
pub(crate) fn boost_to_900mhz() {
    let original_low = unsafe { read64(NODE_PLL_LOW_PADDR) };
    let original_high = unsafe { read64(NODE_PLL_HIGH_PADDR) };
    let original_scale = unsafe { read64(NODE_FREQSCALE_PADDR) };
    let original_mhz = effective_mhz(original_low, original_high, original_scale);
    axplat::console_println!(
        "LS2K1000 node clock before: {:?} MHz low={:#018x} high={:#018x} scale={:#018x}",
        original_mhz,
        original_low,
        original_high,
        original_scale
    );

    unsafe {
        // Linux runtime order: switch to the reference clock first. Combining
        // this with the power-down write can stop the currently selected PLL
        // before the bypass selection has propagated.
        let mut value = read64(NODE_PLL_LOW_PADDR);
        value &= !PLL_SELECT;
        write64(NODE_PLL_LOW_PADDR, value);

        value = read64(NODE_PLL_LOW_PADDR);
        value |= PLL_POWER_DOWN;
        write64(NODE_PLL_LOW_PADDR, value);

        value = read64(NODE_PLL_LOW_PADDR);
        value &= !PLL_SOFT_SET;
        write64(NODE_PLL_LOW_PADDR, value);

        write64(NODE_PLL_LOW_PADDR, TARGET_LOW_CONFIG);
        write64(NODE_PLL_HIGH_PADDR, TARGET_HIGH_CONFIG);

        value = read64(NODE_PLL_LOW_PADDR);
        value |= PLL_SOFT_SET;
        write64(NODE_PLL_LOW_PADDR, value);

        value = read64(NODE_PLL_LOW_PADDR);
        value &= !PLL_POWER_DOWN;
        write64(NODE_PLL_LOW_PADDR, value);
    }

    let mut locked_low = 0;
    for _ in 0..LOCK_POLL_LIMIT {
        locked_low = unsafe { read64(NODE_PLL_LOW_PADDR) };
        if locked_low & PLL_LOCKED != 0 {
            break;
        }
        core::hint::spin_loop();
    }
    if locked_low & PLL_LOCKED == 0 {
        panic!("LS2K1000 900 MHz node PLL failed to lock; hardware watchdog will reset");
    }

    unsafe {
        write64(NODE_PLL_LOW_PADDR, locked_low | PLL_SELECT);
        let scale = read64(NODE_FREQSCALE_PADDR);
        write64(
            NODE_FREQSCALE_PADDR,
            (scale & !0x7) | FULL_FREQSCALE,
        );
    }

    let final_low = unsafe { read64(NODE_PLL_LOW_PADDR) };
    let final_high = unsafe { read64(NODE_PLL_HIGH_PADDR) };
    let final_scale = unsafe { read64(NODE_FREQSCALE_PADDR) };
    let final_mhz = effective_mhz(final_low, final_high, final_scale);
    if final_low & PLL_SELECT == 0 || final_mhz != Some(TARGET_MHZ) {
        panic!(
            "LS2K1000 node clock verification failed: {:?} MHz low={:#018x} high={:#018x} scale={:#018x}",
            final_mhz, final_low, final_high, final_scale
        );
    }
    axplat::console_println!(
        "LS2K1000 node clock ready: {} MHz low={:#018x} high={:#018x} scale={:#018x}",
        TARGET_MHZ,
        final_low,
        final_high,
        final_scale
    );
}
