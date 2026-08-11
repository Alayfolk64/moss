//! Runtime library of [ArceOS](https://github.com/arceos-org/arceos).
//!
//! Any application uses ArceOS should link this library. It does some
//! initialization work before entering the application's `main` function.
//!
//! # Cargo Features
//!
//! - `alloc`: Enable global memory allocator.
//! - `paging`: Enable page table manipulation support.
//! - `irq`: Enable interrupt handling support.
//! - `multitask`: Enable multi-threading support.
//! - `smp`: Enable SMP (symmetric multiprocessing) support.
//! - `fs`: Enable filesystem support.
//! - `net`: Enable networking support.
//! - `display`: Enable graphics support.
//!
//! All the features are optional and disabled by default.

#![cfg_attr(not(test), no_std)]
#![feature(doc_cfg)]

#[macro_use]
extern crate axlog;
#[cfg(feature = "fs")]
extern crate alloc;

#[cfg(all(target_os = "none", not(test)))]
mod lang_items;

#[cfg(feature = "smp")]
mod mp;

#[cfg(feature = "smp")]
pub use self::mp::rust_main_secondary;

const LOGO: &str = r#"
       d8888                            .d88888b.   .d8888b.
      d88888                           d88P" "Y88b d88P  Y88b
     d88P888                           888     888 Y88b.
    d88P 888 888d888  .d8888b  .d88b.  888     888  "Y888b.
   d88P  888 888P"   d88P"    d8P  Y8b 888     888     "Y88b.
  d88P   888 888     888      88888888 888     888       "888
 d8888888888 888     Y88b.    Y8b.     Y88b. .d88P Y88b  d88P
d88P     888 888      "Y8888P  "Y8888   "Y88888P"   "Y8888P"
"#;

unsafe extern "C" {
    /// Application's entry point.
    fn main();
}

struct LogIfImpl;

#[crate_interface::impl_interface]
impl axlog::LogIf for LogIfImpl {
    fn console_write_str(s: &str) {
        axhal::console::write_bytes(s.as_bytes());
    }

    fn current_time() -> core::time::Duration {
        axhal::time::monotonic_time()
    }

    fn current_cpu_id() -> Option<usize> {
        #[cfg(feature = "smp")]
        if is_init_ok() {
            Some(axhal::percpu::this_cpu_id())
        } else {
            None
        }
        #[cfg(not(feature = "smp"))]
        Some(0)
    }

    fn current_task_id() -> Option<u64> {
        if is_init_ok() {
            #[cfg(feature = "multitask")]
            {
                axtask::current_may_uninit().map(|curr| curr.id().as_u64())
            }
            #[cfg(not(feature = "multitask"))]
            None
        } else {
            None
        }
    }
}

#[cfg(feature = "dma")]
struct DmaProtectIfImpl;

#[cfg(feature = "dma")]
#[crate_interface::impl_interface]
impl axdma::DmaProtectIf for DmaProtectIfImpl {
    fn protect_memory(
        vaddr: memory_addr::VirtAddr,
        size: usize,
        flags: axhal::paging::MappingFlags,
    ) -> Result<(), axerrno::AxError> {
        axmm::kernel_aspace().lock().protect(vaddr, size, flags)
    }
}

#[cfg(feature = "driver-dyn")]
struct IoMapIfImpl;

#[cfg(feature = "driver-dyn")]
#[crate_interface::impl_interface]
impl axdriver::dyn_drivers::klib::IoMapIf for IoMapIfImpl {
    fn iomap(
        paddr: memory_addr::PhysAddr,
        size: usize,
    ) -> Result<memory_addr::VirtAddr, axerrno::AxError> {
        axmm::iomap(paddr, size)
    }
}

use core::sync::atomic::{AtomicUsize, Ordering};

static INITED_CPUS: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "fs")]
use alloc::vec::Vec;

#[cfg(feature = "fs")]
use axdriver::prelude::{BaseDriverOps, BlockDriverOps};

#[cfg(feature = "fs")]
static RUNTIME_BLOCK_DEVS: spin::Mutex<Option<Vec<Option<axdriver::AxBlockDevice>>>> =
    spin::Mutex::new(None);

#[cfg(feature = "fs")]
fn set_runtime_block_devices(slots: Vec<Option<axdriver::AxBlockDevice>>) {
    *RUNTIME_BLOCK_DEVS.lock() = Some(slots);
}

#[cfg(feature = "fs")]
pub fn runtime_block_device_count() -> usize {
    RUNTIME_BLOCK_DEVS
        .lock()
        .as_ref()
        .map_or(0, |slots| slots.len())
}

#[cfg(feature = "fs")]
pub fn take_runtime_block_device(slot_idx: usize) -> Option<axdriver::AxBlockDevice> {
    RUNTIME_BLOCK_DEVS
        .lock()
        .as_mut()
        .and_then(|slots| slots.get_mut(slot_idx))
        .and_then(Option::take)
}

fn is_init_ok() -> bool {
    INITED_CPUS.load(Ordering::Acquire) == axhal::cpu_num()
}

/// Publish one bounded boot milestone and reload a firmware/platform watchdog.
///
/// The scheduler-backed progress monitor cannot run during early single-core
/// initialization.  Keeping explicit milestones here preserves the short
/// hardware timeout without making a valid cold boot race a single 30-second
/// deadline covering allocator, MMU, drivers, filesystems and SMP bring-up.
fn boot_watchdog_milestone(_stage: &str) {
    axhal::watchdog::ping();
}

/// The main entry point of the ArceOS runtime.
///
/// It is called from the bootstrapping code in the specific platform crate (see
/// [`axplat::main`]).
///
/// `cpu_id` is the logic ID of the current CPU, and `arg` is passed from the
/// bootloader (typically the device tree blob address).
///
/// In multi-core environment, this function is called on the primary core, and
/// secondary cores call [`rust_main_secondary`].
#[cfg_attr(not(test), axplat::main)]
pub fn rust_main(hardware_cpu_id: usize, arg: usize) -> ! {
    unsafe { axhal::mem::clear_bss() };
    // The device-tree topology always moves the firmware boot CPU to logical
    // slot zero. Establish that per-CPU base before trap/platform setup, whose
    // early paths may already access per-CPU state.
    axhal::percpu::init_primary(0);
    axhal::init_early(hardware_cpu_id, arg);
    let cpu_id = axhal::cpu_logical_id(hardware_cpu_id)
        .expect("boot CPU is missing from the device-tree topology");
    assert_eq!(cpu_id, 0, "boot CPU must occupy logical CPU slot zero");

    ax_println!("{}", LOGO);
    ax_println!(
        indoc::indoc! {"
            arch = {}
            platform = {}
            target = {}
            build_mode = {}
            log_level = {}
            backtrace = {}
            smp = {}
        "},
        axconfig::ARCH,
        axconfig::PLATFORM,
        option_env!("AX_TARGET").unwrap_or(""),
        option_env!("AX_MODE").unwrap_or(""),
        option_env!("AX_LOG").unwrap_or(""),
        axbacktrace::is_enabled(),
        axhal::cpu_num(),
    );
    #[cfg(feature = "rtc")]
    ax_println!(
        "Boot at {}\n",
        chrono::DateTime::from_timestamp_nanos(axhal::time::wall_time_nanos() as _),
    );

    axlog::init();
    axlog::set_max_level(option_env!("AX_LOG").unwrap_or("")); // no effect if set `log-level-*` features
    info!("Logging is enabled.");
    info!("Primary CPU {cpu_id} started, arg = {arg:#x}.");

    axhal::mem::init();
    ax_println!(
        "MOSS_DTB cpu_nodes={} memory_regions={} memory_bytes={:#x}",
        axhal::cpu_num(),
        axhal::mem::phys_ram_ranges().len(),
        axhal::mem::total_ram_size(),
    );
    info!("Found physcial memory regions:");
    for r in axhal::mem::memory_regions() {
        info!(
            "  [{:x?}, {:x?}) {} ({:?})",
            r.paddr,
            r.paddr + r.size,
            r.name,
            r.flags
        );
    }

    #[cfg(feature = "alloc")]
    init_allocator();
    boot_watchdog_milestone("allocator-ready");

    {
        use core::ops::Range;

        unsafe extern "C" {
            safe static _stext: [u8; 0];
            safe static _etext: [u8; 0];
            safe static _edata: [u8; 0];
        }

        let ip_range = Range {
            start: _stext.as_ptr() as usize,
            end: _etext.as_ptr() as usize,
        };

        let fp_range = Range {
            start: _edata.as_ptr() as usize,
            end: usize::MAX,
        };

        axbacktrace::init(ip_range, fp_range);
    }
    boot_watchdog_milestone("backtrace-ready");

    #[cfg(feature = "paging")]
    axmm::init_memory_management();
    boot_watchdog_milestone("paging-ready");

    #[cfg(feature = "driver-dyn")]
    axdriver::setup(arg);
    boot_watchdog_milestone("driver-config-ready");

    info!("Initialize platform devices...");
    axhal::init_later(cpu_id, arg);
    boot_watchdog_milestone("platform-ready");

    #[cfg(feature = "multitask")]
    axtask::init_scheduler();
    boot_watchdog_milestone("scheduler-ready");

    #[cfg(feature = "ipi")]
    axipi::init();

    #[cfg(feature = "irq")]
    {
        info!("Initialize interrupt handlers...");
        init_interrupt();
    }
    boot_watchdog_milestone("interrupts-ready");

    #[cfg(any(feature = "fs", feature = "net", feature = "display"))]
    {
        #[allow(unused_variables)]
        let mut all_devices = axdriver::init_drivers();

        #[cfg(feature = "fs")]
        {
            // In multi-disk contest setups (sdcard + runtime disk), make sure
            // rootfs chooses the main test image instead of the tiny helper disk.
            let mut block_slots: Vec<Option<axdriver::AxBlockDevice>> =
                all_devices.block.drain(..).map(Some).collect();
            assert!(!block_slots.is_empty(), "No block device found!");

            let mut root_idx = 0usize;
            let mut root_bytes = 0u128;
            if block_slots.len() > 1 {
                for (idx, dev_slot) in block_slots.iter().enumerate() {
                    let dev = dev_slot
                        .as_ref()
                        .expect("all block device slots should be occupied");
                    let bytes = dev.num_blocks() as u128 * dev.block_size() as u128;
                    let better_size = bytes > root_bytes;
                    let tie_but_better_name = bytes == root_bytes && {
                        let current = block_slots[root_idx]
                            .as_ref()
                            .expect("root index should point to a valid block device");
                        dev.device_name() < current.device_name()
                    };
                    if better_size || tie_but_better_name {
                        root_bytes = bytes;
                        root_idx = idx;
                    }
                }
                info!(
                    "Select root block device by size: idx={} name={} bytes={}",
                    root_idx,
                    block_slots[root_idx]
                        .as_ref()
                        .expect("root index should point to a valid block device")
                        .device_name(),
                    root_bytes
                );
            }
            let root_dev = block_slots[root_idx]
                .take()
                .expect("root block device slot must exist");
            axfs::init_filesystems(axdriver::AxDeviceContainer::from_one(root_dev));
            set_runtime_block_devices(block_slots);
        }

        #[cfg(feature = "net")]
        axnet::init_network(all_devices.net);
        #[cfg(feature = "vsock")]
        axnet::init_vsock(all_devices.vsock);

        #[cfg(feature = "display")]
        axdisplay::init_display(all_devices.display);

        #[cfg(feature = "input")]
        axinput::init_input(all_devices.input);
    }
    boot_watchdog_milestone("devices-ready");

    #[cfg(feature = "smp")]
    self::mp::start_secondary_cpus(hardware_cpu_id);
    boot_watchdog_milestone("smp-ready");

    #[cfg(all(feature = "tls", not(feature = "multitask")))]
    {
        info!("Initialize thread local storage...");
        init_tls();
    }

    ctor_bare::call_ctors();
    boot_watchdog_milestone("constructors-ready");

    info!("Primary CPU {cpu_id} init OK.");
    INITED_CPUS.fetch_add(1, Ordering::Release);

    while !is_init_ok() {
        core::hint::spin_loop();
    }
    boot_watchdog_milestone("main-entry");

    unsafe { main() };

    #[cfg(feature = "multitask")]
    axtask::exit(0);
    #[cfg(not(feature = "multitask"))]
    {
        debug!("main task exited: exit_code={}", 0);
        axhal::power::system_off();
    }
}

#[cfg(feature = "alloc")]
fn init_allocator() {
    use axhal::mem::{MemRegionFlags, memory_regions, phys_to_virt};

    info!("Initialize global memory allocator...");
    info!("  use {} allocator.", axalloc::global_allocator().name());

    let free_regions = || memory_regions().filter(|r| r.flags.contains(MemRegionFlags::FREE));

    let init_region = free_regions()
        // Bitmap indices are offsets from the initial range. Start at the
        // lowest free address so later, discontiguous RAM banks can be added
        // without moving the bitmap base.
        .min_by_key(|r| r.paddr)
        .expect("no free memory region found!!");

    axalloc::global_init(phys_to_virt(init_region.paddr).as_usize(), init_region.size);

    for r in free_regions() {
        if r.paddr != init_region.paddr {
            axalloc::global_add_memory(phys_to_virt(r.paddr).as_usize(), r.size)
                .expect("add page memory region failed");
        }
    }

    let allocator = axalloc::global_allocator();
    ax_println!(
        "MOSS_MEMORY managed_mib={} free_mib={}",
        (allocator.used_pages() + allocator.available_pages()) * 0x1000 / (1024 * 1024),
        allocator.available_pages() * 0x1000 / (1024 * 1024),
    );
}

#[cfg(feature = "irq")]
fn init_interrupt() {
    // Setup timer interrupt handler
    const PERIODIC_INTERVAL_NANOS: u64 =
        axhal::time::NANOS_PER_SEC / axconfig::TICKS_PER_SEC as u64;

    #[percpu::def_percpu]
    static NEXT_DEADLINE: u64 = 0;

    fn update_timer() {
        let now_ns = axhal::time::monotonic_time_nanos();
        // Safety: we have disabled preemption in IRQ handler.
        let mut deadline = unsafe { NEXT_DEADLINE.read_current_raw() };
        if now_ns >= deadline {
            deadline = now_ns + PERIODIC_INTERVAL_NANOS;
        }
        unsafe { NEXT_DEADLINE.write_current_raw(deadline + PERIODIC_INTERVAL_NANOS) };
        axhal::time::set_oneshot_timer(deadline);
    }

    axhal::irq::register(axhal::time::irq_num(), || {
        update_timer();
        #[cfg(feature = "multitask")]
        axtask::on_timer_tick();
    });

    #[cfg(feature = "ipi")]
    axhal::irq::register(axhal::irq::IPI_IRQ, || {
        axipi::ipi_handler();
    });

    // Prime the first scheduler tick before drivers begin issuing commands
    // that may sleep on interrupt-backed wait queues.
    update_timer();

    // Enable IRQs before driver and filesystem initialization.
    axhal::asm::enable_irqs();
}

#[cfg(all(feature = "tls", not(feature = "multitask")))]
fn init_tls() {
    let main_tls = axhal::tls::TlsArea::alloc();
    unsafe { axhal::asm::write_thread_pointer(main_tls.tls_ptr() as usize) };
    core::mem::forget(main_tls);
}
