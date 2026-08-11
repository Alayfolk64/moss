#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod ahci;
mod ata;
mod completion;
mod controller;
mod hal;
mod host_reset;
mod mmio;
mod port_state;
mod recovery;
mod types;

pub use ahci::{AhciDriver, AhciIrqHandle, PreparedAhci};
pub use hal::Hal;
