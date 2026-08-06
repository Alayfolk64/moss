use axplat::mem::{MemIf, PhysAddr, RawRange, VirtAddr, pa, va};

use crate::config::{
    devices::MMIO_RANGES,
    plat::{
        HIGH_MEMORY_BASE, LOW_MEMORY_BASE, LOW_MEMORY_SIZE, PHYS_BOOT_OFFSET, PHYS_MEMORY_SIZE,
        PHYS_VIRT_OFFSET,
    },
};

struct MemIfImpl;

#[impl_plat_interface]
impl MemIf for MemIfImpl {
    fn phys_ram_ranges() -> &'static [RawRange] {
        const HIGH_MEMORY_SIZE: usize = PHYS_MEMORY_SIZE.saturating_sub(LOW_MEMORY_SIZE);
        &[
            (LOW_MEMORY_BASE, LOW_MEMORY_SIZE),
            (HIGH_MEMORY_BASE, HIGH_MEMORY_SIZE),
        ]
    }

    fn reserved_phys_ram_ranges() -> &'static [RawRange] {
        // Firmware reservations and the actual DTB range are added by axhal.
        // Keep only the conventional low firmware area reserved here.
        &[(0, 0x20_0000)]
    }

    fn mmio_ranges() -> &'static [RawRange] {
        &MMIO_RANGES
    }

    fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        va!(paddr.as_usize() + PHYS_VIRT_OFFSET)
    }

    fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        let value = vaddr.as_usize();
        if value & 0xffff_0000_0000_0000 == PHYS_BOOT_OFFSET {
            pa!(value - PHYS_BOOT_OFFSET)
        } else {
            pa!(value - PHYS_VIRT_OFFSET)
        }
    }

    fn kernel_aspace() -> (VirtAddr, usize) {
        (
            va!(crate::config::plat::KERNEL_ASPACE_BASE),
            crate::config::plat::KERNEL_ASPACE_SIZE,
        )
    }
}
