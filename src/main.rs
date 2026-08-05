#![no_std]
#![no_main]
#![doc = include_str!("../README.md")]

extern crate alloc;

use alloc::{borrow::ToOwned, vec::Vec};

#[cfg(feature = "timerfd-debug-init")]
pub const INIT_SCRIPT: &str = include_str!("init_timerfd_debug.sh");
#[cfg(all(not(feature = "timerfd-debug-init"), feature = "pre-init"))]
pub const INIT_SCRIPT: &str = include_str!("init.sh");
#[cfg(all(
    not(feature = "timerfd-debug-init"),
    not(feature = "pre-init"),
    feature = "shell-init"
))]
pub const INIT_SCRIPT: &str = include_str!("init_shell.sh");
#[cfg(all(
    not(feature = "timerfd-debug-init"),
    not(feature = "pre-init"),
    not(feature = "shell-init")
))]
pub const INIT_SCRIPT: &str = include_str!("init_final.sh");
#[cfg(feature = "pre-init")]
pub const LTP_CASES_RAW: &str = include_str!("ltp_testcases.sh");

#[cfg(any(
    feature = "timerfd-debug-init",
    feature = "pre-init",
    feature = "shell-init"
))]
pub const CMDLINE: &[&str] = &["/musl/busybox", "sh", "-c", INIT_SCRIPT];
#[cfg(not(any(
    feature = "timerfd-debug-init",
    feature = "pre-init",
    feature = "shell-init"
)))]
pub const CMDLINE: &[&str] = &["/bin/bash", "-c", INIT_SCRIPT];

#[cfg(feature = "pre-init")]
fn build_ltp_cases_env() -> alloc::string::String {
    let mut env = alloc::string::String::from("LTP_CASES=");
    let mut first = true;
    for line in LTP_CASES_RAW.lines() {
        let case = line.trim();
        if case.is_empty() || case.starts_with('#') {
            continue;
        }
        if !first {
            env.push('\n');
        }
        first = false;
        env.push_str(case);
    }
    env
}

#[unsafe(no_mangle)]
fn main() {
    let args = CMDLINE
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    #[cfg(any(feature = "pre-init", feature = "shell-init"))]
    let mut envs = Vec::new();
    #[cfg(not(any(feature = "pre-init", feature = "shell-init")))]
    let envs = Vec::new();
    #[cfg(feature = "pre-init")]
    envs.push(build_ltp_cases_env());
    #[cfg(feature = "shell-init")]
    envs.push("STARRY_SHELL_MODE=1".to_owned());

    starry_kernel::entry::init(&args, &envs);
}

#[cfg(feature = "vf2")]
extern crate axplat_riscv64_visionfive2;
