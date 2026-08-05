//! User address space management and user-space memory access.

mod access;
mod aspace;
mod io;
mod loader;
#[cfg(any(target_arch = "loongarch64", target_arch = "riscv64"))]
mod tlb;

#[cfg(any(target_arch = "loongarch64", target_arch = "riscv64"))]
pub(crate) use self::tlb::flush_tlb_on_all_cpus;
#[cfg(any(target_arch = "loongarch64", target_arch = "riscv64"))]
pub use self::tlb::init as init_tlb_shootdown;
pub use self::{access::*, aspace::*, io::*, loader::*};
