use core::mem::size_of;

use axerrno::{AxError, AxResult};
use bytemuck::{Pod, Zeroable};
use starry_vm::{VmMutPtr, VmPtr, vm_load, vm_write_slice};

use crate::config::{USER_SPACE_BASE, USER_SPACE_SIZE};

const RISCV_HWPROBE_WHICH_CPUS: u32 = 1;

const KEY_MVENDORID: i64 = 0;
const KEY_MARCHID: i64 = 1;
const KEY_MIMPID: i64 = 2;
const KEY_BASE_BEHAVIOR: i64 = 3;
const KEY_IMA_EXT_0: i64 = 4;
const KEY_CPUPERF_0: i64 = 5;
const KEY_ZICBOZ_BLOCK_SIZE: i64 = 6;
const KEY_HIGHEST_VIRT_ADDRESS: i64 = 7;
const KEY_TIME_CSR_FREQ: i64 = 8;
const KEY_MISALIGNED_SCALAR_PERF: i64 = 9;
const KEY_MISALIGNED_VECTOR_PERF: i64 = 10;
const KEY_VENDOR_EXT_THEAD_0: i64 = 11;
const KEY_ZICBOM_BLOCK_SIZE: i64 = 12;
const KEY_VENDOR_EXT_SIFIVE_0: i64 = 13;

const BASE_BEHAVIOR_IMA: u64 = 1 << 0;
const IMA_EXT_FD: u64 = 1 << 0;
const IMA_EXT_C: u64 = 1 << 1;
const MISALIGNED_UNKNOWN: u64 = 0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RiscvHwprobe {
    key: i64,
    value: u64,
}

impl RiscvHwprobe {
    fn query(key: i64) -> Self {
        let value = match key {
            // The QEMU virt CPU used by MOSS leaves these IDs at zero.
            KEY_MVENDORID | KEY_MARCHID | KEY_MIMPID => 0,
            KEY_BASE_BEHAVIOR => BASE_BEHAVIOR_IMA,
            KEY_IMA_EXT_0 => IMA_EXT_FD | IMA_EXT_C,
            KEY_CPUPERF_0 | KEY_MISALIGNED_SCALAR_PERF => MISALIGNED_UNKNOWN,
            KEY_ZICBOZ_BLOCK_SIZE | KEY_ZICBOM_BLOCK_SIZE => 0,
            KEY_HIGHEST_VIRT_ADDRESS => (USER_SPACE_BASE + USER_SPACE_SIZE - 1) as u64,
            KEY_TIME_CSR_FREQ => axconfig::devices::TIMER_FREQUENCY as u64,
            KEY_MISALIGNED_VECTOR_PERF => MISALIGNED_UNKNOWN,
            KEY_VENDOR_EXT_THEAD_0 | KEY_VENDOR_EXT_SIFIVE_0 => 0,
            _ => return Self { key: -1, value: 0 },
        };

        Self { key, value }
    }

    fn matches(self, requested: Self) -> bool {
        if self.key != requested.key {
            return false;
        }

        if matches!(
            self.key,
            KEY_BASE_BEHAVIOR
                | KEY_IMA_EXT_0
                | KEY_CPUPERF_0
                | KEY_VENDOR_EXT_THEAD_0
                | KEY_VENDOR_EXT_SIFIVE_0
        ) {
            self.value & requested.value == requested.value
        } else {
            self.value == requested.value
        }
    }
}

pub fn sys_riscv_hwprobe(
    pairs: *mut u8,
    pair_count: usize,
    cpu_set_size: usize,
    cpus: *mut u8,
    flags: u32,
) -> AxResult<isize> {
    let pairs = pairs.cast::<RiscvHwprobe>();

    if flags & RISCV_HWPROBE_WHICH_CPUS != 0 {
        hwprobe_get_cpus(pairs, pair_count, cpu_set_size, cpus, flags)?;
    } else {
        hwprobe_get_values(pairs, pair_count, cpu_set_size, cpus, flags)?;
    }

    Ok(0)
}

fn hwprobe_get_values(
    pairs: *mut RiscvHwprobe,
    pair_count: usize,
    cpu_set_size: usize,
    cpus: *mut u8,
    flags: u32,
) -> AxResult<()> {
    if flags != 0 {
        return Err(AxError::InvalidInput);
    }

    if cpu_set_size != 0 || !cpus.is_null() {
        let mask = load_cpu_mask(cpus, cpu_set_size)?;
        if !mask_has_online_cpu(&mask) {
            return Err(AxError::InvalidInput);
        }
    }

    validate_pair_range(pairs, pair_count)?;
    for index in 0..pair_count {
        let user_pair = pairs.wrapping_add(index);
        let key = user_pair.cast::<i64>().vm_read()?;
        user_pair.vm_write(RiscvHwprobe::query(key))?;
    }

    Ok(())
}

fn hwprobe_get_cpus(
    pairs: *mut RiscvHwprobe,
    pair_count: usize,
    cpu_set_size: usize,
    cpus: *mut u8,
    flags: u32,
) -> AxResult<()> {
    if flags != RISCV_HWPROBE_WHICH_CPUS || cpu_set_size == 0 || cpus.is_null() {
        return Err(AxError::InvalidInput);
    }

    let mut mask = load_cpu_mask(cpus, cpu_set_size)?;
    if mask.iter().all(|byte| *byte == 0) {
        fill_online_cpu_mask(&mut mask);
    } else {
        remove_offline_cpus(&mut mask);
    }

    validate_pair_range(pairs, pair_count)?;
    let mut clear_all = false;
    for index in 0..pair_count {
        let user_pair = pairs.wrapping_add(index);
        let requested = user_pair.vm_read()?;
        let available = RiscvHwprobe::query(requested.key);

        if available.key < 0 {
            clear_all = true;
            user_pair.vm_write(available)?;
        } else if !available.matches(requested) {
            mask.fill(0);
        }
    }

    if clear_all {
        mask.fill(0);
    }
    vm_write_slice(cpus, &mask)?;
    Ok(())
}

fn load_cpu_mask(cpus: *const u8, cpu_set_size: usize) -> AxResult<alloc::vec::Vec<u8>> {
    let bytes = cpu_set_size.min(kernel_cpu_mask_size());
    Ok(vm_load(cpus, bytes)?)
}

fn kernel_cpu_mask_size() -> usize {
    axconfig::plat::CPU_NUM.div_ceil(usize::BITS as usize) * size_of::<usize>()
}

fn mask_has_online_cpu(mask: &[u8]) -> bool {
    (0..axconfig::plat::CPU_NUM).any(|cpu| mask_bit_is_set(mask, cpu))
}

fn fill_online_cpu_mask(mask: &mut [u8]) {
    for cpu in 0..axconfig::plat::CPU_NUM.min(mask.len() * 8) {
        mask[cpu / 8] |= 1 << (cpu % 8);
    }
}

fn remove_offline_cpus(mask: &mut [u8]) {
    for cpu in axconfig::plat::CPU_NUM..mask.len() * 8 {
        mask[cpu / 8] &= !(1 << (cpu % 8));
    }
}

fn mask_bit_is_set(mask: &[u8], cpu: usize) -> bool {
    mask.get(cpu / 8)
        .is_some_and(|byte| byte & (1 << (cpu % 8)) != 0)
}

fn validate_pair_range(pairs: *mut RiscvHwprobe, pair_count: usize) -> AxResult<()> {
    let bytes = pair_count
        .checked_mul(size_of::<RiscvHwprobe>())
        .ok_or(AxError::BadAddress)?;
    pairs.addr().checked_add(bytes).ok_or(AxError::BadAddress)?;
    Ok(())
}
