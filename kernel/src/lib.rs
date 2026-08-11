//! The core functionality of a monolithic kernel, including loading user
//! programs and managing processes.

#![no_std]
#![feature(likely_unlikely)]
#![feature(bstr)]
#![allow(missing_docs)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(dead_code)]

extern crate alloc;
extern crate axruntime;

#[macro_use]
extern crate axlog;

pub mod entry;

mod config;
mod file;
#[cfg(target_arch = "loongarch64")]
mod ipi;
mod mm;
mod pseudofs;
#[cfg(feature = "ls2k1000")]
mod swap;
#[cfg(not(feature = "ls2k1000"))]
#[path = "swap_stub.rs"]
mod swap;
mod syscall;
mod task;
mod time;
#[cfg(feature = "ls2k1000")]
mod watchdog;
