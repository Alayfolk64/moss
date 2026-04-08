#![no_std]
#![no_main]
#![doc = include_str!("../README.md")]

extern crate alloc;

use alloc::{borrow::ToOwned, format, vec::Vec};

pub const CMDLINE: &[&str] = &["/musl/busybox", "sh", "-c", include_str!("init.sh")];
pub const INIT_MODE: &str = if cfg!(feature = "shell-init") {
    "shell"
} else {
    "autotest"
};

#[unsafe(no_mangle)]
fn main() {
    let args = CMDLINE
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let envs = [format!("STARRY_INIT_MODE={INIT_MODE}")];

    starry_kernel::entry::init(&args, &envs);
}

#[cfg(feature = "vf2")]
extern crate axplat_riscv64_visionfive2;
