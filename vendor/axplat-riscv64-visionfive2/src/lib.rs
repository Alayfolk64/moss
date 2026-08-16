#![no_std]

#[macro_use]
extern crate log;
#[macro_use]
extern crate axplat;

mod boot;
mod console;
mod init;
#[cfg(feature = "irq")]
mod irq;
mod mem;
mod power;
mod time;
#[cfg(feature = "watchdog")]
mod watchdog;

pub mod config {
    axconfig_macros::include_configs!(path_env = "AX_CONFIG_PATH", fallback = "axconfig.toml");
    assert_str_eq!(
        PACKAGE,
        env!("CARGO_PKG_NAME"),
        "platform configuration package mismatch"
    );
}
