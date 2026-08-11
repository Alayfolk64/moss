//! Disabled swap backend for platforms without the LS2K1000 swap device.
//!
//! The COW backend is shared by all architectures, so it always references the
//! swap interface.  Non-LS2K1000 builds provide this backend: it never creates
//! a swap slot, which keeps every swap I/O path unreachable at runtime.

use axerrno::{AxError, AxResult};
use memory_addr::PhysAddr;

pub(crate) const fn enabled() -> bool {
    false
}

pub(crate) const fn reserve_slot() -> Option<usize> {
    None
}

pub(crate) const fn retain_slot(_slot: usize) -> bool {
    false
}

pub(crate) const fn release_slot(_slot: usize) {}

pub(crate) fn write_page(_slot: usize, _paddr: PhysAddr) -> AxResult {
    Err(AxError::OperationNotSupported)
}

pub(crate) fn read_page(_slot: usize, _paddr: PhysAddr) -> AxResult {
    Err(AxError::OperationNotSupported)
}

pub(crate) const fn note_working_set_probes(_pages: usize) {}

pub(crate) const fn note_working_set_refault() {}
