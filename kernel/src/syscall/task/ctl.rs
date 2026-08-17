use core::ffi::c_char;

use axerrno::{AxError, AxResult, LinuxError};
use axsync::Mutex;
use axtask::current;
use hashbrown::HashMap;
use lazy_static::lazy_static;
use linux_raw_sys::general::{__user_cap_data_struct, __user_cap_header_struct, CAP_LAST_CAP};
use starry_signal::Signo;
use starry_vm::{VmMutPtr, VmPtr, vm_write_slice};

use crate::{
    mm::vm_load_string,
    task::{AsThread, get_process_data, get_task},
};

const CAPABILITY_VERSION_1: u32 = 0x19980330;
const CAPABILITY_VERSION_2: u32 = 0x20071026;
const CAPABILITY_VERSION_3: u32 = 0x20080522;
const CAP_SETPCAP: u32 = 8;
const CAP_MASK: u64 = (1u64 << (CAP_LAST_CAP + 1)) - 1;
const SECUREBITS_VALID_MASK: u32 = 0xff;
const SECBIT_NO_CAP_AMBIENT_RAISE: u32 = 1 << 6;

#[derive(Clone, Copy, Debug)]
struct CapState {
    effective: u64,
    permitted: u64,
    inheritable: u64,
    bounding: u64,
    ambient: u64,
    securebits: u32,
}

impl Default for CapState {
    fn default() -> Self {
        Self {
            effective: u32::MAX as u64,
            permitted: u32::MAX as u64,
            inheritable: 0,
            bounding: CAP_MASK,
            ambient: 0,
            securebits: 0,
        }
    }
}

lazy_static! {
    static ref CAP_STATE: Mutex<HashMap<u32, CapState>> = Mutex::new(HashMap::new());
}

fn current_pid() -> u32 {
    current().as_thread().proc_data.proc.pid()
}

#[inline]
fn eperm() -> AxError {
    AxError::from(LinuxError::EPERM)
}

fn get_state(pid: u32) -> CapState {
    let mut all = CAP_STATE.lock();
    *all.entry(pid).or_default()
}

pub fn current_has_cap(cap: u32) -> bool {
    cap < 64 && (get_state(current_pid()).effective & (1u64 << cap)) != 0
}

fn set_state(pid: u32, state: CapState) {
    CAP_STATE.lock().insert(pid, state);
}

pub fn release_process_cap_state(pid: u32) {
    CAP_STATE.lock().remove(&pid);
}

pub fn inherit_process_cap_state(parent_pid: u32, child_pid: u32) {
    let parent = get_state(parent_pid);
    set_state(child_pid, parent);
}

fn read_caps(version: u32, data: *mut __user_cap_data_struct) -> AxResult<(u64, u64, u64)> {
    let d0 = unsafe { data.vm_read_uninit()?.assume_init() };
    let d1 = if version == CAPABILITY_VERSION_1 {
        __user_cap_data_struct {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        }
    } else {
        unsafe { data.wrapping_add(1).vm_read_uninit()?.assume_init() }
    };
    let merge = |lo: u32, hi: u32| (lo as u64) | ((hi as u64) << 32);
    Ok((
        merge(d0.effective, d1.effective),
        merge(d0.permitted, d1.permitted),
        merge(d0.inheritable, d1.inheritable),
    ))
}

fn write_caps(version: u32, data: *mut __user_cap_data_struct, caps: CapState) -> AxResult {
    let split = |bits: u64| (bits as u32, (bits >> 32) as u32);
    let (e0, e1) = split(caps.effective);
    let (p0, p1) = split(caps.permitted);
    let (i0, i1) = split(caps.inheritable);

    data.vm_write(__user_cap_data_struct {
        effective: e0,
        permitted: p0,
        inheritable: i0,
    })?;
    if version != CAPABILITY_VERSION_1 {
        data.wrapping_add(1).vm_write(__user_cap_data_struct {
            effective: e1,
            permitted: p1,
            inheritable: i1,
        })?;
    }
    Ok(())
}

fn validate_cap_header(header_ptr: *mut __user_cap_header_struct) -> AxResult<(u32, u32)> {
    // FIXME: AnyBitPattern
    let mut header = unsafe { header_ptr.vm_read_uninit()?.assume_init() };

    let version = match header.version {
        CAPABILITY_VERSION_1 | CAPABILITY_VERSION_2 | CAPABILITY_VERSION_3 => header.version,
        _ => {
            header.version = CAPABILITY_VERSION_3;
            header_ptr.vm_write(header)?;
            return Err(AxError::InvalidInput);
        }
    };

    if header.pid < 0 {
        return Err(AxError::InvalidInput);
    }
    let target_pid = if header.pid == 0 {
        current_pid()
    } else {
        let id = header.pid as u32;
        get_process_data(id)
            .map(|proc_data| proc_data.proc.pid())
            .or_else(|_| get_task(id).map(|task| task.as_thread().proc_data.proc.pid()))?
    };
    Ok((version, target_pid))
}

pub fn sys_capget(
    header: *mut __user_cap_header_struct,
    data: *mut __user_cap_data_struct,
) -> AxResult<isize> {
    let (version, target_pid) = validate_cap_header(header)?;
    let caps = get_state(target_pid);
    write_caps(version, data, caps)?;
    Ok(0)
}

pub fn sys_capset(
    header: *mut __user_cap_header_struct,
    data: *mut __user_cap_data_struct,
) -> AxResult<isize> {
    let (version, target_pid) = validate_cap_header(header)?;
    let pid = current_pid();
    if target_pid != pid {
        return Err(eperm());
    }

    let old = get_state(pid);
    let (new_e, new_p, new_i) = read_caps(version, data)?;

    if new_e & !new_p != 0 {
        return Err(eperm());
    }
    if new_p & !old.permitted != 0 {
        return Err(eperm());
    }
    // Linux: inherited new bits must stay inside the bounding set.
    if (new_i & !old.inheritable) & !old.bounding != 0 {
        return Err(eperm());
    }
    // Without CAP_SETPCAP, new inheritable bits must come from old permitted/inheritable.
    let has_setpcap = (old.effective & (1u64 << CAP_SETPCAP)) != 0;
    if !has_setpcap && (new_i & !(old.inheritable | old.permitted) != 0) {
        return Err(eperm());
    }

    set_state(
        pid,
        CapState {
            effective: new_e,
            permitted: new_p,
            inheritable: new_i,
            bounding: old.bounding,
            ambient: old.ambient & new_p & new_i,
            securebits: old.securebits,
        },
    );

    Ok(0)
}

pub fn sys_umask(mask: u32) -> AxResult<isize> {
    let curr = current();
    let old = curr.as_thread().proc_data.replace_umask(mask);
    Ok(old as isize)
}

const UID_GID_NO_CHANGE: u32 = u32::MAX;

fn valid_id_change(new_id: u32, allowed_ids: &[u32]) -> bool {
    new_id == UID_GID_NO_CHANGE || allowed_ids.contains(&new_id)
}

pub fn sys_setreuid(ruid: u32, euid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let old_ruid = proc_data.uid();
    let old_euid = proc_data.euid();
    let old_suid = proc_data.suid();
    let new_euid = if euid != UID_GID_NO_CHANGE {
        euid
    } else {
        old_euid
    };

    if old_euid != 0
        && (!valid_id_change(ruid, &[old_ruid, old_euid])
            || !valid_id_change(euid, &[old_ruid, old_euid, old_suid]))
    {
        return Err(eperm());
    }
    if ruid != UID_GID_NO_CHANGE {
        proc_data.set_real_uid(ruid);
    }
    if euid != UID_GID_NO_CHANGE {
        proc_data.set_euid(euid);
    }
    if ruid != UID_GID_NO_CHANGE || (euid != UID_GID_NO_CHANGE && euid != old_ruid) {
        proc_data.set_suid(new_euid);
    }
    Ok(0)
}

pub fn sys_setregid(rgid: u32, egid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let old_rgid = proc_data.gid();
    let old_egid = proc_data.egid();
    let old_sgid = proc_data.sgid();
    let new_egid = if egid != UID_GID_NO_CHANGE {
        egid
    } else {
        old_egid
    };

    if proc_data.euid() != 0
        && (!valid_id_change(rgid, &[old_rgid, old_egid])
            || !valid_id_change(egid, &[old_rgid, old_egid, old_sgid]))
    {
        return Err(eperm());
    }
    if rgid != UID_GID_NO_CHANGE {
        proc_data.set_real_gid(rgid);
    }
    if egid != UID_GID_NO_CHANGE {
        proc_data.set_egid(egid);
    }
    if rgid != UID_GID_NO_CHANGE || (egid != UID_GID_NO_CHANGE && egid != old_rgid) {
        proc_data.set_sgid(new_egid);
    }
    Ok(0)
}

pub fn sys_setresuid(ruid: u32, euid: u32, _suid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let old_ruid = proc_data.uid();
    let old_euid = proc_data.euid();
    let old_suid = proc_data.suid();

    if old_euid != 0 {
        let allowed = [old_ruid, old_euid, old_suid];
        if !valid_id_change(ruid, &allowed)
            || !valid_id_change(euid, &allowed)
            || !valid_id_change(_suid, &allowed)
        {
            return Err(eperm());
        }
    }
    if ruid != UID_GID_NO_CHANGE {
        proc_data.set_real_uid(ruid);
    }
    if euid != UID_GID_NO_CHANGE {
        proc_data.set_euid(euid);
    }
    if _suid != UID_GID_NO_CHANGE {
        proc_data.set_suid(_suid);
    }
    Ok(0)
}

pub fn sys_setresgid(rgid: u32, egid: u32, _sgid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let old_rgid = proc_data.gid();
    let old_egid = proc_data.egid();
    let old_sgid = proc_data.sgid();

    if proc_data.euid() != 0 {
        let allowed = [old_rgid, old_egid, old_sgid];
        if !valid_id_change(rgid, &allowed)
            || !valid_id_change(egid, &allowed)
            || !valid_id_change(_sgid, &allowed)
        {
            return Err(eperm());
        }
    }
    if rgid != UID_GID_NO_CHANGE {
        proc_data.set_real_gid(rgid);
    }
    if egid != UID_GID_NO_CHANGE {
        proc_data.set_egid(egid);
    }
    if _sgid != UID_GID_NO_CHANGE {
        proc_data.set_sgid(_sgid);
    }
    Ok(0)
}

pub fn sys_setfsuid(uid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let old = proc_data.fsuid();
    if uid != UID_GID_NO_CHANGE
        && (proc_data.euid() == 0
            || [proc_data.uid(), proc_data.euid(), proc_data.suid(), old].contains(&uid))
    {
        proc_data.set_fsuid(uid);
    }
    // Linux reports the previous fsuid even when the requested change is not
    // permitted; callers query success by issuing a second setfsuid(-1).
    Ok(old as isize)
}

pub fn sys_setfsgid(gid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let old = proc_data.fsgid();
    let in_groups = proc_data.supplementary_groups().contains(&gid);
    if gid != UID_GID_NO_CHANGE
        && (proc_data.euid() == 0
            || in_groups
            || [proc_data.gid(), proc_data.egid(), proc_data.sgid(), old].contains(&gid))
    {
        proc_data.set_fsgid(gid);
    }
    Ok(old as isize)
}

pub fn sys_get_mempolicy(
    _policy: *mut i32,
    _nodemask: *mut usize,
    _maxnode: usize,
    _addr: usize,
    _flags: usize,
) -> AxResult<isize> {
    warn!("Dummy get_mempolicy called");
    Ok(0)
}

/// prctl() is called with a first argument describing what to do, and further
/// arguments with a significance depending on the first one.
/// The first argument can be:
/// - PR_SET_NAME: set the name of the calling thread, using the value pointed to by `arg2`
/// - PR_GET_NAME: get the name of the calling
/// - PR_SET_SECCOMP: enable seccomp mode, with the mode specified in `arg2`
/// - PR_MCE_KILL: set the machine check exception policy
/// - PR_SET_MM options: set various memory management options (start/end code/data/brk/stack)
pub fn sys_prctl(
    option: u32,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> AxResult<isize> {
    use linux_raw_sys::prctl::*;

    debug!("sys_prctl <= option: {option}, args: {arg2}, {arg3}, {arg4}, {arg5}");

    match option {
        PR_SET_NAME => {
            let s = vm_load_string(arg2 as *const c_char)?;
            current().set_name(&s);
        }
        PR_GET_NAME => {
            let name = current().name();
            let len = name.len().min(15);
            let mut buf = [0; 16];
            buf[..len].copy_from_slice(&name.as_bytes()[..len]);
            vm_write_slice(arg2 as _, &buf)?;
        }
        PR_SET_DUMPABLE => {
            if !matches!(arg2, 0 | 1) {
                return Err(AxError::InvalidInput);
            }
            // The unused variadic arguments are deliberately ignored.  Many C
            // callers pass only option and value, so those registers are not a
            // valid part of PR_SET_DUMPABLE's ABI.
            current().as_thread().proc_data.set_dumpable(arg2 != 0);
        }
        PR_GET_DUMPABLE => {
            return Ok(current().as_thread().proc_data.dumpable() as isize);
        }
        PR_SET_NO_NEW_PRIVS => {
            if arg2 != 1 || arg3 != 0 || arg4 != 0 || arg5 != 0 {
                return Err(AxError::InvalidInput);
            }
            current().as_thread().proc_data.set_no_new_privs();
        }
        PR_GET_NO_NEW_PRIVS => {
            if arg2 != 0 || arg3 != 0 || arg4 != 0 || arg5 != 0 {
                return Err(AxError::InvalidInput);
            }
            return Ok(current().as_thread().proc_data.no_new_privs() as isize);
        }
        PR_SET_SECCOMP => {}
        PR_MCE_KILL => {}
        PR_GET_PDEATHSIG => {
            let signo = current().as_thread().pdeath_signal();
            let ptr = arg2 as *mut i32;
            ptr.vm_write(signo as i32)?;
        }
        PR_SET_PDEATHSIG => {
            if arg2 != 0 {
                let signo = u8::try_from(arg2).map_err(|_| AxError::InvalidInput)?;
                Signo::from_repr(signo).ok_or(AxError::InvalidInput)?;
            }
            current().as_thread().set_pdeath_signal(arg2 as u32);
        }
        PR_GET_SECUREBITS => {
            if arg2 != 0 || arg3 != 0 || arg4 != 0 || arg5 != 0 {
                return Err(AxError::InvalidInput);
            }
            return Ok(get_state(current_pid()).securebits as isize);
        }
        PR_SET_SECUREBITS => {
            if arg3 != 0 || arg4 != 0 || arg5 != 0 || arg2 & !(SECUREBITS_VALID_MASK as usize) != 0
            {
                return Err(AxError::InvalidInput);
            }
            if !current_has_cap(CAP_SETPCAP) {
                return Err(eperm());
            }

            let pid = current_pid();
            let mut state = get_state(pid);
            let new_securebits = arg2 as u32;
            for value_bit in [0u32, 2, 4, 6] {
                let pair_mask = 3u32 << value_bit;
                let lock_bit = 1u32 << (value_bit + 1);
                if state.securebits & lock_bit != 0
                    && (state.securebits ^ new_securebits) & pair_mask != 0
                {
                    return Err(eperm());
                }
            }
            state.securebits = new_securebits;
            set_state(pid, state);
        }
        PR_CAP_AMBIENT => {
            if arg4 != 0 || arg5 != 0 {
                return Err(AxError::InvalidInput);
            }

            let pid = current_pid();
            let mut state = get_state(pid);
            match arg2 as u32 {
                PR_CAP_AMBIENT_IS_SET | PR_CAP_AMBIENT_RAISE | PR_CAP_AMBIENT_LOWER => {
                    let cap = u32::try_from(arg3).map_err(|_| AxError::InvalidInput)?;
                    if cap > CAP_LAST_CAP {
                        return Err(AxError::InvalidInput);
                    }
                    let bit = 1u64 << cap;
                    match arg2 as u32 {
                        PR_CAP_AMBIENT_IS_SET => return Ok((state.ambient & bit != 0) as isize),
                        PR_CAP_AMBIENT_RAISE => {
                            if state.permitted & bit == 0
                                || state.inheritable & bit == 0
                                || state.securebits & SECBIT_NO_CAP_AMBIENT_RAISE != 0
                            {
                                return Err(eperm());
                            }
                            state.ambient |= bit;
                        }
                        PR_CAP_AMBIENT_LOWER => state.ambient &= !bit,
                        _ => unreachable!(),
                    }
                }
                PR_CAP_AMBIENT_CLEAR_ALL => {
                    if arg3 != 0 {
                        return Err(AxError::InvalidInput);
                    }
                    state.ambient = 0;
                }
                _ => return Err(AxError::InvalidInput),
            }
            set_state(pid, state);
        }
        PR_CAPBSET_READ => {
            let cap = arg2 as u32;

            if cap >= 64 {
                return Err(AxError::InvalidInput);
            }

            let pid = current_pid();
            let state = get_state(pid);
            let is_set = (state.bounding & (1u64 << cap)) != 0;

            return Ok(is_set as isize);
        }
        PR_CAPBSET_DROP => {
            let cap = arg2 as u32;
            if cap >= 64 {
                return Err(AxError::InvalidInput);
            }

            if !current_has_cap(CAP_SETPCAP) {
                return Err(eperm());
            }

            let pid = current_pid();
            let mut state = get_state(pid);

            state.bounding &= !(1u64 << cap);
            set_state(pid, state);
        }
        PR_SET_MM => {
            // not implemented; but avoid annoying warnings
            return Err(AxError::InvalidInput);
        }
        PR_SET_THP_DISABLE => {
            // MOSS only creates huge mappings for an explicit MAP_HUGETLB
            // request, so transparent huge pages are effectively always
            // disabled.  Validate Linux's arguments and report success.
            if arg2 > 1 || arg3 != 0 || arg4 != 0 || arg5 != 0 {
                return Err(AxError::InvalidInput);
            }
        }
        PR_GET_THP_DISABLE => {
            if arg2 != 0 || arg3 != 0 || arg4 != 0 || arg5 != 0 {
                return Err(AxError::InvalidInput);
            }
            return Ok(1);
        }
        _ => {
            warn!("sys_prctl: unsupported option {option}");
            return Err(AxError::InvalidInput);
        }
    }

    Ok(0)
}
