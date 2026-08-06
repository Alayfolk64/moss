//! Physical memory management.

use heapless::Vec;
use lazyinit::LazyInit;

use axplat::mem::{RawRange, check_sorted_ranges_overlap, ranges_difference};
use memory_addr::{align_down_4k, align_up_4k};

pub use axplat::mem::{MemRegionFlags, PhysMemRegion};
pub use axplat::mem::{mmio_ranges, phys_to_virt, virt_to_phys};
// `kernel_aspace` is only available in newer (unpublished) versions of axplat;
// the crates.io 0.3.0 release does not have it.
#[cfg(any(feature = "defplat", feature = "myplat"))]
pub use axplat::mem::kernel_aspace;
pub use memory_addr::{PAGE_SIZE_4K, PhysAddr, PhysAddrRange, VirtAddr, VirtAddrRange, pa, va};

use crate::addr_of_sym;

const MAX_REGIONS: usize = 128;

static ALL_MEM_REGIONS: LazyInit<Vec<PhysMemRegion, MAX_REGIONS>> = LazyInit::new();
static PHYS_RAM_RANGES: LazyInit<Vec<RawRange, MAX_REGIONS>> = LazyInit::new();
static RESERVED_RAM_RANGES: LazyInit<Vec<RawRange, MAX_REGIONS>> = LazyInit::new();

fn checked_end(start: usize, size: usize, kind: &str) -> usize {
    start
        .checked_add(size)
        .unwrap_or_else(|| panic!("{kind} range overflows: start={start:#x}, size={size:#x}"))
}

fn push_ram_range(ranges: &mut Vec<RawRange, MAX_REGIONS>, start: usize, size: usize) {
    if size == 0 {
        return;
    }
    assert!(
        start <= usize::MAX - (PAGE_SIZE_4K - 1),
        "RAM range start cannot be page-aligned: {start:#x}"
    );
    let end = align_down_4k(checked_end(start, size, "RAM"));
    let start = align_up_4k(start);
    assert!(start < end, "RAM range is empty after page alignment");
    ranges
        .push((start, end - start))
        .expect("too many RAM ranges in device tree");
}

fn merge_sorted_ranges(mut ranges: Vec<RawRange, MAX_REGIONS>) -> Vec<RawRange, MAX_REGIONS> {
    ranges.sort_unstable_by_key(|&(start, _)| start);
    let mut merged = Vec::<RawRange, MAX_REGIONS>::new();

    for (start, size) in ranges {
        let end = checked_end(start, size, "physical memory");
        if let Some(last_index) = merged.len().checked_sub(1) {
            let (last_start, last_size) = merged[last_index];
            let last_end = checked_end(last_start, last_size, "physical memory");
            if start <= last_end {
                merged[last_index].1 = end.max(last_end) - last_start;
                continue;
            }
        }
        merged
            .push((start, size))
            .expect("too many merged physical memory ranges");
    }
    merged
}

fn discover_ram_ranges() -> Vec<RawRange, MAX_REGIONS> {
    let mut ranges = Vec::new();

    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    {
        let fdt = crate::dtb::get_fdt().expect("QEMU requires a valid device tree");
        for memory in fdt.memory() {
            for region in memory.regions() {
                push_ram_range(&mut ranges, region.address as usize, region.size);
            }
        }
        assert!(!ranges.is_empty(), "device tree contains no usable RAM");
    }

    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    for &(start, size) in axplat::mem::phys_ram_ranges() {
        push_ram_range(&mut ranges, start, size);
    }

    ranges.sort_unstable_by_key(|&(start, _)| start);
    check_sorted_ranges_overlap(ranges.iter().copied())
        .inspect_err(|(a, b)| error!("DT RAM region {a:#x?} overlaps with {b:#x?}"))
        .expect("overlapping RAM ranges in device tree");

    // Adjacent memory nodes describe one allocatable span.
    merge_sorted_ranges(ranges)
}

fn push_reserved_range(ranges: &mut Vec<RawRange, MAX_REGIONS>, start: usize, size: usize) {
    if size == 0 {
        return;
    }
    let raw_end = checked_end(start, size, "reserved memory");
    assert!(
        raw_end <= usize::MAX - (PAGE_SIZE_4K - 1),
        "reserved range end cannot be page-aligned: {raw_end:#x}"
    );
    let start = align_down_4k(start);
    let end = align_up_4k(raw_end);
    ranges
        .push((start, end - start))
        .expect("too many reserved memory ranges");
}

fn discover_reserved_ranges() -> Vec<RawRange, MAX_REGIONS> {
    let mut ranges = Vec::new();
    for &(start, size) in axplat::mem::reserved_phys_ram_ranges() {
        push_reserved_range(&mut ranges, start, size);
    }

    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    {
        use fdt_parser::Status;

        let fdt = crate::dtb::get_fdt().expect("QEMU requires a valid device tree");
        for region in fdt.memory_reservation_block() {
            push_reserved_range(&mut ranges, region.address as usize, region.size);
        }

        // fdt-parser exposes the /reserved-memory parent, so walk its direct
        // children and collect every enabled `reg` tuple.
        let mut parent_level = None;
        for node in fdt.all_nodes() {
            if let Some(level) = parent_level {
                if node.level <= level {
                    parent_level = None;
                } else if node.level == level + 1 && node.status() != Some(Status::Disabled) {
                    if let Some(regions) = node.reg() {
                        for region in regions {
                            push_reserved_range(
                                &mut ranges,
                                region.address as usize,
                                region.size.unwrap_or_default(),
                            );
                        }
                    }
                }
            }
            if node.name().split('@').next() == Some("reserved-memory") {
                parent_level = Some(node.level);
            }
        }

        if let Some((start, size)) = crate::dtb::physical_range() {
            push_reserved_range(&mut ranges, start, size);
        }
    }

    merge_sorted_ranges(ranges)
}

/// Returns the page-aligned RAM ranges discovered from the boot device tree.
pub fn phys_ram_ranges() -> &'static [RawRange] {
    if let Some(ranges) = PHYS_RAM_RANGES.get() {
        return ranges.as_slice();
    }
    PHYS_RAM_RANGES.init_once(discover_ram_ranges()).as_slice()
}

/// Returns the merged platform and device-tree reserved memory ranges.
pub fn reserved_phys_ram_ranges() -> &'static [RawRange] {
    if let Some(ranges) = RESERVED_RAM_RANGES.get() {
        return ranges.as_slice();
    }
    RESERVED_RAM_RANGES
        .init_once(discover_reserved_ranges())
        .as_slice()
}

/// Returns the total RAM size reported by the boot device tree.
pub fn total_ram_size() -> usize {
    phys_ram_ranges().iter().map(|range| range.1).sum()
}

/// Returns an iterator over all physical memory regions.
pub fn memory_regions() -> impl Iterator<Item = PhysMemRegion> {
    ALL_MEM_REGIONS.iter().cloned()
}

/// Fills the `.bss` section with zeros.
///
/// It requires the symbols `_sbss` and `_ebss` to be defined in the linker script.
///
/// # Safety
///
/// This function is unsafe because it writes `.bss` section directly.
pub unsafe fn clear_bss() {
    unsafe {
        core::slice::from_raw_parts_mut(
            _sbss as *mut u8,
            (_ebss as *mut u8).offset_from_unsigned(_sbss as *mut u8),
        )
        .fill(0);
    }
}

/// Initializes physical memory regions.
pub fn init() {
    let mut all_regions = Vec::new();
    let mut push = |r: PhysMemRegion| {
        if r.size > 0 {
            all_regions.push(r).expect("too many memory regions");
        }
    };

    // Push regions in kernel image
    push(PhysMemRegion {
        paddr: virt_to_phys(addr_of_sym!(_stext).into()),
        size: addr_of_sym!(_etext) - addr_of_sym!(_stext),
        flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::EXECUTE,
        name: ".text",
    });
    push(PhysMemRegion {
        paddr: virt_to_phys(addr_of_sym!(_srodata).into()),
        size: addr_of_sym!(_erodata) - addr_of_sym!(_srodata),
        flags: MemRegionFlags::RESERVED | MemRegionFlags::READ,
        name: ".rodata",
    });
    push(PhysMemRegion {
        paddr: virt_to_phys(addr_of_sym!(_sdata).into()),
        size: addr_of_sym!(_sdma_uncached) - addr_of_sym!(_sdata),
        flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
        name: ".data",
    });
    push(PhysMemRegion {
        paddr: virt_to_phys(addr_of_sym!(_sdma_uncached).into()),
        size: addr_of_sym!(_edma_uncached) - addr_of_sym!(_sdma_uncached),
        flags: MemRegionFlags::RESERVED
            | MemRegionFlags::READ
            | MemRegionFlags::WRITE
            | MemRegionFlags::UNCACHED,
        name: ".dma_uncached",
    });
    push(PhysMemRegion {
        paddr: virt_to_phys(addr_of_sym!(_edma_uncached).into()),
        size: addr_of_sym!(_edata) - addr_of_sym!(_edma_uncached),
        flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
        name: ".tdata .tbss .percpu",
    });
    push(PhysMemRegion {
        paddr: virt_to_phys(addr_of_sym!(boot_stack).into()),
        size: addr_of_sym!(boot_stack_top) - addr_of_sym!(boot_stack),
        flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
        name: "boot stack",
    });
    push(PhysMemRegion {
        paddr: virt_to_phys(addr_of_sym!(_sbss).into()),
        size: addr_of_sym!(_ebss) - addr_of_sym!(_sbss),
        flags: MemRegionFlags::RESERVED | MemRegionFlags::READ | MemRegionFlags::WRITE,
        name: ".bss",
    });

    // Push MMIO & reserved regions
    for &(start, size) in mmio_ranges() {
        push(PhysMemRegion::new_mmio(start, size, "mmio"));
    }
    for &(start, size) in reserved_phys_ram_ranges() {
        push(PhysMemRegion::new_reserved(start, size, "reserved"));
    }

    // Combine kernel image range and reserved ranges
    let kernel_start = virt_to_phys(addr_of_sym!(_skernel).into()).as_usize();
    let kernel_size = addr_of_sym!(_ekernel) - addr_of_sym!(_skernel);
    let mut reserved_ranges = reserved_phys_ram_ranges()
        .iter()
        .cloned()
        .chain(core::iter::once((kernel_start, kernel_size))) // kernel image range is also reserved
        .collect::<Vec<_, MAX_REGIONS>>();

    // Remove all reserved ranges from RAM ranges, and push the remaining as free memory
    reserved_ranges.sort_unstable_by_key(|&(start, _size)| start);
    ranges_difference(phys_ram_ranges(), &reserved_ranges, |(start, size)| {
        push(PhysMemRegion::new_ram(start, size, "free memory"));
    })
    .inspect_err(|(a, b)| error!("Reserved memory region {a:#x?} overlaps with {b:#x?}"))
    .unwrap();

    // Check overlapping
    all_regions.sort_unstable_by_key(|r| r.paddr);
    check_sorted_ranges_overlap(all_regions.iter().map(|r| (r.paddr.into(), r.size)))
        .inspect_err(|(a, b)| error!("Physical memory region {a:#x?} overlaps with {b:#x?}"))
        .unwrap();

    ALL_MEM_REGIONS.init_once(all_regions);
}

unsafe extern "C" {
    fn _stext();
    fn _etext();
    fn _srodata();
    fn _erodata();
    fn _sdata();
    fn _sdma_uncached();
    fn _edma_uncached();
    fn _edata();
    fn _sbss();
    fn _ebss();
    fn _skernel();
    fn _ekernel();
    fn boot_stack();
    fn boot_stack_top();
}
