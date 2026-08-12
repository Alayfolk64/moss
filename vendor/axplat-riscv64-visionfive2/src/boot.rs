use axplat::mem::{Aligned4K, pa};

use crate::config::plat::{BOOT_STACK_SIZE, PHYS_VIRT_OFFSET};

#[unsafe(link_section = ".bss.stack")]
static mut BOOT_STACK: [u8; BOOT_STACK_SIZE] = [0; BOOT_STACK_SIZE];

#[unsafe(link_section = ".data")]
static mut BOOT_PT_SV39: Aligned4K<[u64; 512]> = Aligned4K::new([0; 512]);

#[allow(clippy::identity_op)]
unsafe fn init_boot_page_table() {
    unsafe {
        // Keep both identity and higher-half mappings while switching on Sv39.
        // The 4-GiB VF2 RAM spans 0x4000_0000..0x1_4000_0000, and U-Boot
        // relocates its control DTB near the top of the first 4 GiB. Map the
        // complete physical span needed to parse that DTB before the final
        // kernel address space is installed.
        BOOT_PT_SV39[0] = (0x0 << 10) | 0xef;
        BOOT_PT_SV39[1] = (0x40000 << 10) | 0xef;
        BOOT_PT_SV39[2] = (0x80000 << 10) | 0xef;
        BOOT_PT_SV39[3] = (0xc0000 << 10) | 0xef;
        BOOT_PT_SV39[4] = (0x100000 << 10) | 0xef;
        BOOT_PT_SV39[0x100] = (0x0 << 10) | 0xef;
        BOOT_PT_SV39[0x101] = (0x40000 << 10) | 0xef;
        BOOT_PT_SV39[0x102] = (0x80000 << 10) | 0xef;
        BOOT_PT_SV39[0x103] = (0xc0000 << 10) | 0xef;
        BOOT_PT_SV39[0x104] = (0x100000 << 10) | 0xef;
    }
}

unsafe fn init_mmu() {
    unsafe {
        axcpu::asm::write_boot_page_table(pa!(&raw const BOOT_PT_SV39 as usize));
        axcpu::asm::flush_tlb(None);
    }
}

/// Earliest entry point. OpenSBI starts VF2 in S-mode on physical hart 1.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "
        csrci   sstatus, 2              // mask supervisor IRQs during early boot
        csrwi   sie, 0                  // discard interrupt enables left by U-Boot
        li      s0, 1                   // VF2 OpenSBI boot hart
        li      s1, 0x42200000          // OpenSBI DTB retained by U-Boot
        la      sp, {boot_stack}
        li      t0, {boot_stack_size}
        add     sp, sp, t0

        call    {init_boot_page_table}
        call    {init_mmu}

        li      s2, {phys_virt_offset}
        add     sp, sp, s2

        mv      a0, s0                 // runtime maps physical hart 1 to logical CPU 0
        mv      a1, s1
        la      a2, {entry}
        add     a2, a2, s2
        jalr    a2
        j       .",
        phys_virt_offset = const PHYS_VIRT_OFFSET,
        boot_stack_size = const BOOT_STACK_SIZE,
        boot_stack = sym BOOT_STACK,
        init_boot_page_table = sym init_boot_page_table,
        init_mmu = sym init_mmu,
        entry = sym axplat::call_main,
    )
}

/// Earliest entry point for a U74 core started through SBI HSM.
#[cfg(feature = "smp")]
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn _start_secondary() -> ! {
    // a0 = physical hart ID
    // a1 = physical address of this hart's boot-stack top
    core::arch::naked_asm!(
        "
        csrci   sstatus, 2
        csrwi   sie, 0
        mv      s0, a0
        mv      sp, a1

        call    {init_mmu}

        li      s1, {phys_virt_offset}
        add     a1, a1, s1
        add     sp, sp, s1

        mv      a0, s0
        la      a1, {entry}
        add     a1, a1, s1
        jalr    a1
        j       .",
        phys_virt_offset = const PHYS_VIRT_OFFSET,
        init_mmu = sym init_mmu,
        entry = sym axplat::call_secondary_main,
    )
}
