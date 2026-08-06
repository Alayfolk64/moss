#![no_std]

extern crate alloc;

mod ahci;
mod ata;
mod completion;
mod hal;
mod mmio;
mod types;

pub use ahci::{AhciDriver, AhciIrqHandle, PreparedAhci};
pub use hal::Hal;
