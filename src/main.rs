#![no_std]
#![no_main]
#![doc = include_str!("../README.md")]

extern crate alloc;

use alloc::{borrow::ToOwned, vec::Vec};
use starry_kernel::runtime_cpu_count;
use alloc::string::String;

const INIT_PRE: &str   = include_str!("init_pre.sh");
const INIT_SHELL: &str = include_str!("init_shell.sh");
const INIT_FINAL: &str = include_str!("init_final.sh");
const INIT_SITE: &str = include_str!("init_site.sh");

#[cfg(all(feature = "shell-init", feature = "site-init"))]
compile_error!("shell-init and site-init cannot be enabled at the same time");

#[derive(Debug, Clone, Copy)]
enum InitProfile {
    Pre,
    Final,
    Shell,
    Site,
}

impl InitProfile {
    fn name(&self) -> &'static str {
        match self {
            InitProfile::Pre   => "pre",
            InitProfile::Shell => "shell",
            InitProfile::Final => "final",
            InitProfile::Site  => "site",
        }
    }
}

fn select_init_profile() -> InitProfile {
    #[cfg(feature = "shell-init")]
    {
        return InitProfile::Shell;
    }

    #[cfg(feature = "site-init")]
    {
        return InitProfile::Site;
    }

    if runtime_cpu_count() == 1 {
        InitProfile::Pre
    } else {
        InitProfile::Final
    }
}

fn build_args(profile: InitProfile) -> Vec<String> {
    let cmdline: &[&str] = match profile {
        InitProfile::Pre => &[
            "/musl/busybox",
            "sh",
            "-c",
            INIT_PRE,
        ],
        InitProfile::Final => &[
            "/bin/bash",
            "-c",
            INIT_FINAL,
        ],
        InitProfile::Shell => &[
            "/bin/sh",
            "-c",
            INIT_SHELL,
        ],
        InitProfile::Site => &[
            "/bin/sh",
            "-c",
            INIT_SITE,
        ],
    };
    cmdline
        .iter()
        .copied()
        .map(str::to_owned)
        .collect()
}

pub const LTP_CASES_RAW: &str = include_str!("ltp_testcases.sh");


fn build_ltp_cases_env() -> String {
    let mut env = String::from("LTP_CASES=");
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
    let profile = select_init_profile();
    let cpu_num = runtime_cpu_count();

    let args = build_args(profile);
    let mut envs = Vec::new();

    match profile {
        InitProfile::Pre => {
            envs.push(build_ltp_cases_env());
        }
        InitProfile::Shell => {
            envs.push("STARRY_SHELL_MODE=1".to_owned());
        }
        InitProfile::Final | InitProfile::Site => {}
    }
    starry_kernel::entry::init(&args, &envs);
}

#[cfg(feature = "vf2")]
extern crate axplat_riscv64_visionfive2;

#[cfg(feature = "ls2k1000")]
extern crate axplat_loongarch64_ls2k1000;
