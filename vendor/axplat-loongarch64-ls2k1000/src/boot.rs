use core::ptr::NonNull;

use axplat::mem::{Aligned4K, pa, va};
use fdt_parser::Fdt;
use page_table_entry::{GenericPTE, MappingFlags, loongarch64::LA64PTE};

use crate::config::plat::{
    BOOT_STACK_SIZE, HIGH_MEMORY_BASE, KERNEL_BASE_PADDR, LOW_MEMORY_SIZE, PHYS_BOOT_OFFSET,
    PHYS_MEMORY_SIZE, PHYS_VIRT_OFFSET,
};

const L1_BLOCK_SIZE: usize = 1 << 30;
const KERNEL_L0_INDEX: usize = (PHYS_VIRT_OFFSET >> 39) & 0x1ff;
const BOOT_TO_VIRT: usize = PHYS_VIRT_OFFSET - PHYS_BOOT_OFFSET;
const UNCACHED_DMW_INIT: usize = crate::fdt::UNCACHED_DMW_BASE | 0x1;
const CACHED_DMW_INIT: usize = PHYS_BOOT_OFFSET | 0x11;
const UHI_FDT_ARG0: usize = usize::MAX - 1;
const MAX_UBOOT_GO_ARGS: usize = 16;

#[unsafe(link_section = ".bss.stack")]
static mut BOOT_STACK: [u8; BOOT_STACK_SIZE] = [0; BOOT_STACK_SIZE];

#[unsafe(link_section = ".data.boot_page_table")]
static mut BOOT_PT_L0: Aligned4K<[LA64PTE; 512]> = Aligned4K::new([LA64PTE::empty(); 512]);
#[unsafe(link_section = ".data.boot_page_table")]
static mut BOOT_PT_L1: Aligned4K<[LA64PTE; 512]> = Aligned4K::new([LA64PTE::empty(); 512]);
#[unsafe(link_section = ".data.boot_page_table")]
static mut BOOT_PT_L2: Aligned4K<[LA64PTE; 512]> = Aligned4K::new([LA64PTE::empty(); 512]);

unsafe fn init_boot_page_table() {
    unsafe {
        let l1_va = va!(&raw const BOOT_PT_L1 as usize);
        BOOT_PT_L0[KERNEL_L0_INDEX] = LA64PTE::new_table(axplat::mem::virt_to_phys(l1_va));
        let l2_va = va!(&raw const BOOT_PT_L2 as usize);
        BOOT_PT_L1[0] = LA64PTE::new_table(axplat::mem::virt_to_phys(l2_va));

        // Bootstrap only: map the address windows needed to reach the DTB and
        // linked kernel. The allocator's definitive RAM map comes from DTB.
        for i in 0..512 {
            BOOT_PT_L2[i] = LA64PTE::new_page(
                pa!(i << 21),
                MappingFlags::READ
                    | MappingFlags::WRITE
                    | if i < 128 {
                        MappingFlags::EXECUTE
                    } else {
                        MappingFlags::DEVICE
                    },
                true,
            );
        }
        let high_memory_size = PHYS_MEMORY_SIZE.saturating_sub(LOW_MEMORY_SIZE);
        let first_block = HIGH_MEMORY_BASE / L1_BLOCK_SIZE;
        let last_block = (HIGH_MEMORY_BASE + high_memory_size).div_ceil(L1_BLOCK_SIZE);
        for block in first_block..last_block {
            BOOT_PT_L1[block] = LA64PTE::new_page(
                pa!(block * L1_BLOCK_SIZE),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
                true,
            );
        }
    }
}

fn enable_fp_simd() {
    #[cfg(feature = "fp-simd")]
    {
        axcpu::asm::enable_fp();
        axcpu::asm::enable_lsx();
    }
}

fn init_mmu() {
    axcpu::init::init_mmu(
        axplat::mem::virt_to_phys(va!(&raw const BOOT_PT_L0 as usize)),
        PHYS_BOOT_OFFSET,
    );
}

unsafe fn parse_uboot_addr_arg(arg: usize) -> Option<usize> {
    if arg == 0 {
        return None;
    }
    let ptr = crate::fdt::uncached_addr(crate::fdt::to_phys(arg)) as *const u8;
    unsafe { crate::boot_arg::parse_hex_c_str(ptr) }
}

unsafe fn uboot_fdt_arg(argc: usize, argv: usize) -> Option<usize> {
    if !(2..=MAX_UBOOT_GO_ARGS).contains(&argc) || argv == 0 {
        return None;
    }
    let argv = crate::fdt::uncached_addr(crate::fdt::to_phys(argv)) as *const usize;
    // Reject random small integers that happen to resemble argc. argv[0]
    // must itself be the hexadecimal `go` entry address.
    unsafe { parse_uboot_addr_arg(argv.read())? };
    for index in 1..argc {
        if let Some(address) = unsafe { parse_uboot_addr_arg(argv.add(index).read()) } {
            return Some(address);
        }
    }
    None
}

fn valid_fdt_paddr(candidate: usize) -> Option<usize> {
    let paddr = crate::fdt::to_phys(candidate);
    let ptr = NonNull::new(crate::fdt::uncached_addr(paddr) as *mut u8)?;
    Fdt::from_ptr(ptr).ok().map(|_| paddr)
}

extern "C" fn resolve_fdt_arg(arg0: usize, arg1: usize) -> usize {
    let candidate = if arg0 == UHI_FDT_ARG0 {
        Some(arg1)
    } else {
        unsafe { uboot_fdt_arg(arg0, arg1) }
    };
    candidate
        .and_then(valid_fdt_paddr)
        .expect("U-Boot/UHI did not provide a valid DTB")
}

unsafe extern "C" {
    static mut _sbss: u8;
    static mut _ebss: u8;
}

/// Raw image entry. The image remains linked for the board's U-Boot load
/// address, while all runtime topology and devices come from the supplied DTB.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "
        .word   0x5a4d
        .word   0
        .dword  {kernel_base_paddr} + 0x40
        .dword  _ekernel - _skernel
        .dword  {kernel_base_paddr}
        .dword  0
        .dword  0
        .dword  0
        .word   0x818223cd
        .word   0

        // Linux enters LoongArch with CRMD.IE cleared before touching any
        // firmware-owned state.  Do the same here: U-Boot is not required to
        // leave the interrupt-enable bit in a particular state, and our trap
        // vector/handler tables are not installed yet.
        ori         $t0, $zero, 4
        csrxchg     $zero, $t0, 0x0

        move        $s0, $a0
        move        $s1, $a1

        li.d        $t0, {uncached_dmw_init}
        csrwr       $t0, 0x180
        li.d        $t0, {cached_dmw_init}
        csrwr       $t0, 0x181

        la.local    $t0, 1f
        li.d        $t1, {phys_boot_offset}
        or          $t0, $t0, $t1
        jirl        $zero, $t0, 0

    1:
        la.local    $t0, {sbss}
        la.local    $t1, {ebss}
    2:
        bgeu        $t0, $t1, 3f
        st.d        $zero, $t0, 0
        addi.d      $t0, $t0, 8
        b           2b
    3:
        la.local    $sp, {boot_stack}
        li.d        $t0, {boot_stack_size}
        add.d       $sp, $sp, $t0

        bl          {enable_fp_simd}
        bl          {init_boot_page_table}
        bl          {init_mmu}

        li.d        $t0, {boot_to_virt}
        add.d       $sp, $sp, $t0

        move        $a0, $s0
        move        $a1, $s1
        bl          {resolve_fdt_arg}

        move        $a1, $a0
        csrrd       $a0, 0x20
        la.abs      $t0, {entry}
        li.d        $ra, 0
        jirl        $zero, $t0, 0",
        kernel_base_paddr = const KERNEL_BASE_PADDR,
        phys_boot_offset = const PHYS_BOOT_OFFSET,
        uncached_dmw_init = const UNCACHED_DMW_INIT,
        cached_dmw_init = const CACHED_DMW_INIT,
        boot_to_virt = const BOOT_TO_VIRT,
        boot_stack = sym BOOT_STACK,
        boot_stack_size = const BOOT_STACK_SIZE,
        sbss = sym _sbss,
        ebss = sym _ebss,
        enable_fp_simd = sym enable_fp_simd,
        init_boot_page_table = sym init_boot_page_table,
        init_mmu = sym init_mmu,
        resolve_fdt_arg = sym resolve_fdt_arg,
        entry = sym axplat::call_main,
    )
}

#[cfg(feature = "smp")]
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn _start_secondary() -> ! {
    core::arch::naked_asm!(
        "
        ori         $t0, $zero, 4
        csrxchg     $zero, $t0, 0x0

        li.w        $t0, 0x1028
        iocsrrd.d   $sp, $t0

        li.d        $t0, {uncached_dmw_init}
        csrwr       $t0, 0x180
        li.d        $t0, {cached_dmw_init}
        csrwr       $t0, 0x181

        bl          {enable_fp_simd}
        bl          {init_mmu}

        li.d        $t0, {boot_to_virt}
        add.d       $sp, $sp, $t0

        csrrd       $a0, 0x20
        la.abs      $t0, {entry}
        li.d        $ra, 0
        jirl        $zero, $t0, 0",
        uncached_dmw_init = const UNCACHED_DMW_INIT,
        cached_dmw_init = const CACHED_DMW_INIT,
        boot_to_virt = const BOOT_TO_VIRT,
        enable_fp_simd = sym enable_fp_simd,
        init_mmu = sym init_mmu,
        entry = sym axplat::call_secondary_main,
    )
}
