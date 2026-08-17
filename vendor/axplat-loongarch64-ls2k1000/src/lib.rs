#![no_std]

#[macro_use]
extern crate log;
#[macro_use]
extern crate axplat;

pub mod config {
    axconfig_macros::include_configs!(path_env = "AX_CONFIG_PATH", fallback = "axconfig.toml");
    assert_str_eq!(
        PACKAGE,
        env!("CARGO_PKG_NAME"),
        "platform configuration package mismatch"
    );
}

mod boot;
mod boot_arg;
mod console;
mod fdt;
mod init;
mod ipi_state;
#[cfg(feature = "irq")]
mod irq;
#[cfg(feature = "irq")]
mod irq_state;
mod mem;
mod mem_layout;
#[cfg(feature = "smp")]
mod mp;
mod node_clock;
mod node_clock_state;
mod power;
mod rtc_state;
mod time;
mod watchdog;
