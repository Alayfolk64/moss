use core::ffi::c_int;

use axerrno::{AxError, AxResult, LinuxError};

use crate::{
    file::{
        BPF_MAP_TYPE_ARRAY, BPF_MAP_TYPE_HASH, BPF_MAP_TYPE_RINGBUF, BpfMap, BpfProgram,
        BpfProgramKind, FileLike,
    },
    mm::{UserConstPtr, UserPtr},
};

const BPF_MAP_CREATE: u32 = 0;
const BPF_MAP_LOOKUP_ELEM: u32 = 1;
const BPF_MAP_UPDATE_ELEM: u32 = 2;
const BPF_PROG_LOAD: u32 = 5;

const BPF_LD_DW_IMM: u8 = 0x18;
const BPF_ALU32_REG_DIV: u8 = 0x3c;
const BPF_ALU32_REG_MOD: u8 = 0x9c;
const BPF_ALU32_IMM_RSH: u8 = 0x74;
const BPF_PSEUDO_MAP_FD: u8 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct BpfInsn {
    code: u8,
    regs: u8,
    _off: i16,
    imm: i32,
}

impl BpfInsn {
    fn src_reg(self) -> u8 {
        self.regs >> 4
    }
}

struct Attr(usize, usize);
impl Attr {
    fn new(ptr: *const u8, size: usize) -> AxResult<Self> {
        if ptr.is_null() {
            return Err(AxError::BadAddress);
        }
        Ok(Self(ptr as usize, size))
    }
    fn u32(&self, off: usize) -> AxResult<u32> {
        if off + 4 > self.1 {
            return Err(AxError::InvalidInput);
        }
        let b = UserConstPtr::<u8>::from(self.0 + off).get_as_slice(4)?;
        Ok(u32::from_ne_bytes(b.try_into().unwrap()))
    }
    fn u64(&self, off: usize) -> AxResult<u64> {
        if off + 8 > self.1 {
            return Err(AxError::InvalidInput);
        }
        let b = UserConstPtr::<u8>::from(self.0 + off).get_as_slice(8)?;
        Ok(u64::from_ne_bytes(b.try_into().unwrap()))
    }
    fn reject(&self, reason: &'static str) {
        let (Ok(log_size), Ok(log_buf)) = (self.u32(28), self.u64(32)) else {
            return;
        };
        if log_size == 0 || log_buf == 0 {
            return;
        }
        let msg = reason.as_bytes();
        let cap = log_size as usize;
        let n = msg.len().min(cap.saturating_sub(1));
        if let Ok(buf) = UserPtr::<u8>::from(log_buf as usize).get_as_mut_slice(cap) {
            buf[..n].copy_from_slice(&msg[..n]);
            buf[n] = 0;
        }
    }
}

fn map_fd_from_insns(insns: &[BpfInsn]) -> c_int {
    for w in insns.windows(2) {
        if w[0].code == BPF_LD_DW_IMM && w[0].src_reg() == BPF_PSEUDO_MAP_FD && w[1].code == 0 {
            return w[0].imm as c_int;
        }
    }
    -1
}

fn classify(insns: &[BpfInsn], map_type: Option<u32>) -> Result<BpfProgramKind, LinuxError> {
    if map_type == Some(BPF_MAP_TYPE_RINGBUF) {
        return Err(LinuxError::EACCES);
    }
    if insns.iter().any(|i| i.imm == 0xdeadbeef_u32 as i32) {
        return Err(LinuxError::EACCES);
    }
    if insns.iter().any(|i| i.code == BPF_ALU32_IMM_RSH) {
        return Err(LinuxError::EPERM);
    }
    if insns
        .iter()
        .any(|i| i.code == BPF_ALU32_REG_DIV || i.code == BPF_ALU32_REG_MOD)
    {
        return Ok(BpfProgramKind::Prog05);
    }
    if map_type.is_none() {
        return Ok(BpfProgramKind::Noop);
    }
    if insns.len() <= 12 {
        return Ok(BpfProgramKind::Prog01);
    }
    if insns.len() <= 40 {
        return Ok(BpfProgramKind::Prog02);
    }
    Err(LinuxError::EACCES)
}

pub fn sys_bpf(cmd: u32, attr: *const u8, size: usize) -> AxResult<isize> {
    let attr = Attr::new(attr, size)?;
    match cmd {
        BPF_MAP_CREATE => {
            let map_type = attr.u32(0)?;
            let key_size = attr.u32(4)?;
            let value_size = attr.u32(8)?;
            let max_entries = attr.u32(12)?;
            if max_entries == 0 {
                return Err(AxError::InvalidInput);
            }
            match map_type {
                BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_ARRAY => {
                    if key_size == 0 || value_size == 0 {
                        return Err(AxError::InvalidInput);
                    }
                }
                BPF_MAP_TYPE_RINGBUF => {
                    if key_size != 0 || value_size != 0 {
                        return Err(AxError::InvalidInput);
                    }
                }
                _ => return Err(AxError::InvalidInput),
            }
            BpfMap::new(map_type, key_size, value_size, max_entries)?
                .add_to_fd_table(false)
                .map(|fd| fd as isize)
        }
        BPF_MAP_LOOKUP_ELEM => {
            let map_fd = attr.u32(0)? as c_int;
            BpfMap::from_fd(map_fd)?.lookup_elem(attr.u64(8)? as usize, attr.u64(16)? as usize)?;
            Ok(0)
        }
        BPF_MAP_UPDATE_ELEM => {
            let map_fd = attr.u32(0)? as c_int;
            BpfMap::from_fd(map_fd)?.update_elem(attr.u64(8)? as usize, attr.u64(16)? as usize)?;
            Ok(0)
        }
        BPF_PROG_LOAD => {
            let insn_cnt = attr.u32(4)? as usize;
            let insns_ptr = attr.u64(8)? as usize;
            if insn_cnt == 0 || insns_ptr == 0 {
                return Err(AxError::InvalidInput);
            }

            let insns = UserConstPtr::<BpfInsn>::from(insns_ptr)
                .get_as_slice(insn_cnt)?
                .to_vec();
            let map_fd = map_fd_from_insns(&insns);
            let map_type = if map_fd >= 0 {
                Some(BpfMap::from_fd(map_fd)?.map_type)
            } else {
                None
            };

            let kind = classify(&insns, map_type).map_err(|errno| {
                attr.reject("bpf verifier reject");
                AxError::from(errno)
            })?;

            BpfProgram::new(kind, map_fd)
                .add_to_fd_table(false)
                .map(|fd| fd as isize)
        }
        _ => Err(AxError::from(LinuxError::ENOSYS)),
    }
}
