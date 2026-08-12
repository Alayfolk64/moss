#![doc = include_str!("../README.md")]
#![no_std]
#![warn(missing_docs)]

mod cmd;
mod idmac;
mod interrupt;
mod regs;
mod sdmmc;
mod utils;

pub use self::{
    idmac::DmaConfig,
    interrupt::{InterruptMailbox, InterruptWait, acknowledge_interrupt},
    sdmmc::{SdMmc, SdMmcError, set_interrupt_enabled},
};
