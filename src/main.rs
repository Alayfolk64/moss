#![no_std]
#![no_main]
#![doc = include_str!("../README.md")]

extern crate alloc;

use alloc::{borrow::ToOwned, vec::Vec};

#[cfg(feature = "shell-init")]
pub const INIT_SCRIPT: &str = include_str!("init-shell.sh");
#[cfg(not(feature = "shell-init"))]
pub const INIT_SCRIPT: &str = include_str!("init.sh");

pub const CMDLINE: &[&str] = &["/musl/busybox", "sh", "-c", INIT_SCRIPT];

#[unsafe(no_mangle)]
fn main() {
    let args = CMDLINE
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let envs = [];

    starry_kernel::entry::init(&args, &envs);
}

#[cfg(feature = "vf2")]
extern crate axplat_riscv64_visionfive2;
