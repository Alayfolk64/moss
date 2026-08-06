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
mod console;
mod fdt;
mod init;
#[cfg(feature = "irq")]
mod irq;
mod mem;
#[cfg(feature = "smp")]
mod mp;
mod power;
mod time;
