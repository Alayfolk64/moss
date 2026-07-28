//! User address space management and user-space memory access.

#[cfg(feature = "fault-repro-trace")]
use memory_addr::MemoryAddr;

mod access;
mod aspace;
mod io;
mod loader;
#[cfg(any(target_arch = "loongarch64", target_arch = "riscv64"))]
mod tlb;

#[cfg(feature = "fault-repro-trace")]
pub(crate) const PAGE_FAULT_REPRO_ADDRESS: usize = 0x30_0000_0000;

#[cfg(feature = "fault-repro-trace")]
pub(crate) fn is_page_fault_repro_address(addr: memory_addr::VirtAddr) -> bool {
    addr.align_down(memory_addr::PAGE_SIZE_4K).as_usize() == PAGE_FAULT_REPRO_ADDRESS
}

#[cfg(any(target_arch = "loongarch64", target_arch = "riscv64"))]
pub use self::tlb::init as init_tlb_shootdown;
pub use self::{access::*, aspace::*, io::*, loader::*};
