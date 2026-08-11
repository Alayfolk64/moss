use axplat::mem::{MemIf, PhysAddr, RawRange, VirtAddr, pa, va};

use crate::{
    config::{
        devices::MMIO_RANGES,
        plat::{
            HIGH_MEMORY_BASE, LOW_MEMORY_BASE, LOW_MEMORY_SIZE, PHYS_BOOT_OFFSET, PHYS_MEMORY_SIZE,
            PHYS_VIRT_OFFSET,
        },
    },
    mem_layout::top_reservation,
};

// U-Boot's bdinfo/LMB map reserves the last 16 MiB of the 256 MiB low bank.
// The preceding framebuffer reservation is described by the DTB, but this
// firmware workspace is not.
const FIRMWARE_LOW_RESERVED: RawRange =
    match top_reservation(LOW_MEMORY_BASE, LOW_MEMORY_SIZE, 0x0100_0000) {
        Some(range) => range,
        None => panic!("LS2K1000 firmware reservation exceeds the low RAM bank"),
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
        // DTB reservations and the actual DTB range are added by axhal.
        &[(LOW_MEMORY_BASE, 0x20_0000), FIRMWARE_LOW_RESERVED]
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
