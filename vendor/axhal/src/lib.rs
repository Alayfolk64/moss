//! [ArceOS] hardware abstraction layer, provides unified APIs for
//! platform-specific operations.
//!
//! It does the bootstrapping and initialization process for the specified
//! platform, and provides useful operations on the hardware.
//!
//! Currently supported platforms (specify by cargo features):
//!
//! - `x86-pc`: Standard PC with x86_64 ISA.
//! - `riscv64-qemu-virt`: QEMU virt machine with RISC-V ISA.
//! - `aarch64-qemu-virt`: QEMU virt machine with AArch64 ISA.
//! - `aarch64-raspi`: Raspberry Pi with AArch64 ISA.
//! - `dummy`: If none of the above platform is selected, the dummy platform
//!   will be used. In this platform, most of the operations are no-op or
//!   `unimplemented!()`. This platform is mainly used for [cargo test].
//!
//! # Cargo Features
//!
//! - `smp`: Enable SMP (symmetric multiprocessing) support.
//! - `fp-simd`: Enable floating-point and SIMD support.
//! - `paging`: Enable page table manipulation.
//! - `irq`: Enable interrupt handling support.
//! - `tls`: Enable kernel space thread-local storage support.
//! - `rtc`: Enable real-time clock support.
//! - `uspace`: Enable user space support.
//!
//! [ArceOS]: https://github.com/arceos-org/arceos
//! [cargo test]: https://doc.rust-lang.org/cargo/guide/tests.html

#![no_std]
#![feature(doc_cfg)]

#[allow(unused_imports)]
#[macro_use]
extern crate log;

#[allow(unused_imports)]
#[macro_use]
extern crate memory_addr;

cfg_if::cfg_if! {
    if #[cfg(feature = "myplat")] {
        // link the custom platform crate in your application.
    } else if #[cfg(all(target_os = "none", feature = "defplat"))] {
        #[cfg(target_arch = "x86_64")]
        extern crate axplat_x86_pc;
        #[cfg(target_arch = "aarch64")]
        extern crate axplat_aarch64_qemu_virt;
        #[cfg(target_arch = "riscv64")]
        extern crate axplat_riscv64_qemu_virt;
        #[cfg(target_arch = "loongarch64")]
        extern crate axplat_loongarch64_qemu_virt;
    } else {
        // Link the dummy platform implementation to pass cargo test.
        mod dummy;
    }
}

pub mod dtb;
pub mod mem;
pub mod percpu;
pub mod time;

#[cfg(feature = "tls")]
pub mod tls;

#[cfg(feature = "irq")]
pub mod irq;

#[cfg(feature = "paging")]
pub mod paging;

/// Console input and output.
pub mod console {
    #[cfg(all(feature = "irq", any(feature = "defplat", feature = "myplat")))]
    pub use axplat::console::irq_num;
    #[cfg(all(feature = "irq", not(any(feature = "defplat", feature = "myplat"))))]
    pub fn irq_num() -> Option<usize> {
        None
    }
    pub use axplat::console::{read_bytes, write_bytes};
}

/// CPU power management.
pub mod power {
    #[cfg(feature = "smp")]
    pub use axplat::power::cpu_boot;
    pub use axplat::power::{system_off, system_reset};
}

/// Trap handling.
pub mod trap {
    pub use axcpu::trap::{IRQ, PAGE_FAULT, PageFaultFlags, register_trap_handler};
}

/// CPU register states for context switching.
///
/// There are two types of context:
///
/// - [`TaskContext`][axcpu::TaskContext]: The context of a task.
/// - [`TrapFrame`][axcpu::TrapFrame]: The context of an interrupt or an exception.
pub mod context {
    pub use axcpu::{TaskContext, TrapFrame};
}

pub use axcpu::asm;

#[cfg(feature = "uspace")]
pub use axcpu::uspace;

pub use axplat::init::init_later;

#[cfg(feature = "smp")]
pub use axplat::init::{init_early_secondary, init_later_secondary};

/// Initializes the platform and boot argument.
/// This function should be called as early as possible.
pub fn init_early(cpu_id: usize, arg: usize) {
    dtb::init(arg, cpu_id);
    axplat::init::init_early(cpu_id, arg);
}

/// Returns the number of CPUs available at runtime.
///
/// The device tree is authoritative. `axconfig::plat::CPU_NUM` only sizes
/// fixed per-CPU storage and therefore acts as a hard capacity.
pub fn cpu_num() -> usize {
    #[cfg(feature = "smp")]
    {
        use lazyinit::LazyInit;

        static CPU_NUM: LazyInit<usize> = LazyInit::new();
        if let Some(cpu_num) = CPU_NUM.get() {
            return *cpu_num;
        }

        let capacity = axconfig::plat::CPU_NUM;
        let detected =
            dtb::cpu_count().expect("SMP requires a valid CPU topology in the device tree");
        assert!(
            detected <= capacity,
            "device tree declares {detected} CPUs, exceeding build capacity {capacity}"
        );
        *CPU_NUM.init_once(detected)
    }
    #[cfg(not(feature = "smp"))]
    {
        1
    }
}

/// Translates a dense logical CPU index to its firmware-visible identifier.
pub fn cpu_hardware_id(logical_id: usize) -> usize {
    *dtb::cpu_hardware_ids()
        .expect("CPU topology is unavailable")
        .get(logical_id)
        .expect("logical CPU ID is out of range")
}

/// Translates a firmware-visible CPU identifier to a dense logical index.
pub fn cpu_logical_id(hardware_id: usize) -> Option<usize> {
    dtb::cpu_hardware_ids()?
        .iter()
        .take(cpu_num())
        .position(|id| *id == hardware_id)
}

macro_rules! addr_of_sym {
    ($e:ident) => {
        $e as *const () as usize
    };
}
pub(crate) use addr_of_sym;
