use alloc::{collections::BTreeMap, string::String, vec, vec::Vec};
use core::{
    ffi::c_char,
    fmt::Write as _,
    mem::MaybeUninit,
    sync::atomic::{AtomicI32, Ordering},
};

use axconfig::ARCH;
use axerrno::{AxError, AxResult, LinuxError};
use axfs::FS_CONTEXT;
use axfs_ng_vfs::{NodeType, path::Path};
use axsync::Mutex;
use linux_raw_sys::{
    general::{GRND_INSECURE, GRND_NONBLOCK, GRND_RANDOM, MS_RDONLY},
    system::{new_utsname, sysinfo},
};
use starry_vm::{VmMutPtr, vm_load, vm_read_slice, vm_write_slice};

use crate::{
    file::{resolve_at, with_fs},
    mm::vm_load_string,
    pseudofs::dev::fill_random_bytes,
    task::{AsThread, processes},
};

static NEXT_KEY_SERIAL: AtomicI32 = AtomicI32::new(1000);
static KEY_QUOTAS: Mutex<BTreeMap<u32, KeyQuota>> = Mutex::new(BTreeMap::new());
static ACCT_FILE: Mutex<Option<String>> = Mutex::new(None);

const KEY_MAX_KEYS: usize = 200;
const KEY_MAX_BYTES: usize = 20000;

const KEYCTL_GET_KEYRING_ID: i32 = 0;
const KEYCTL_JOIN_SESSION_KEYRING: i32 = 1;
const KEYCTL_UPDATE: i32 = 2;
const KEYCTL_REVOKE: i32 = 3;
const KEYCTL_SETPERM: i32 = 5;
const KEYCTL_CLEAR: i32 = 7;
const KEYCTL_UNLINK: i32 = 9;
const KEYCTL_READ: i32 = 11;
const KEYCTL_SET_REQKEY_KEYRING: i32 = 14;
const KEYCTL_SET_TIMEOUT: i32 = 15;
const KEYCTL_INVALIDATE: i32 = 21;

const KEY_SPEC_THREAD_KEYRING: i32 = -1;
const KEY_SPEC_PROCESS_KEYRING: i32 = -2;
const KEY_SPEC_SESSION_KEYRING: i32 = -3;
const KEY_SPEC_USER_KEYRING: i32 = -4;
const KEY_SPEC_USER_SESSION_KEYRING: i32 = -5;

#[derive(Default)]
struct KeyQuota {
    used_keys: usize,
    used_bytes: usize,
}

pub fn sys_getresgid(rid: *mut u32, eid: *mut u32, sid: *mut u32) -> AxResult<isize> {
    let curr = axtask::current();
    let proc_data = &curr.as_thread().proc_data;
    rid.vm_write(proc_data.gid())?;
    eid.vm_write(proc_data.egid())?;
    sid.vm_write(proc_data.sgid())?;
    Ok(0)
}

pub fn sys_getresuid(rid: *mut u32, eid: *mut u32, sid: *mut u32) -> AxResult<isize> {
    let curr = axtask::current();
    let proc_data = &curr.as_thread().proc_data;
    rid.vm_write(proc_data.uid())?;
    eid.vm_write(proc_data.euid())?;
    sid.vm_write(proc_data.suid())?;
    Ok(0)
}

pub fn sys_getuid() -> AxResult<isize> {
    Ok(axtask::current().as_thread().proc_data.uid() as _)
}

pub fn sys_geteuid() -> AxResult<isize> {
    Ok(axtask::current().as_thread().proc_data.euid() as _)
}

pub fn sys_getgid() -> AxResult<isize> {
    Ok(axtask::current().as_thread().proc_data.gid() as _)
}

pub fn sys_getegid() -> AxResult<isize> {
    Ok(axtask::current().as_thread().proc_data.egid() as _)
}

pub fn sys_setuid(uid: u32) -> AxResult<isize> {
    debug!("sys_setuid <= uid: {uid}");
    let curr = axtask::current();
    let proc_data = &curr.as_thread().proc_data;
    if proc_data.euid() == 0 {
        proc_data.set_uid(uid);
    } else if uid == proc_data.uid() || uid == proc_data.suid() {
        proc_data.set_euid(uid);
    } else {
        return Err(AxError::OperationNotPermitted);
    }
    Ok(0)
}

pub fn sys_setgid(gid: u32) -> AxResult<isize> {
    debug!("sys_setgid <= gid: {gid}");
    let curr = axtask::current();
    let proc_data = &curr.as_thread().proc_data;
    if proc_data.euid() == 0 {
        proc_data.set_gid(gid);
    } else if gid == proc_data.gid() || gid == proc_data.sgid() {
        proc_data.set_egid(gid);
    } else {
        return Err(AxError::OperationNotPermitted);
    }
    Ok(0)
}

pub fn sys_personality(personality: usize) -> AxResult<isize> {
    const PERSONALITY_GET: usize = 0xffff_ffff;

    let curr = axtask::current();
    let proc_data = &curr.as_thread().proc_data;
    let old = proc_data.personality();
    if personality != PERSONALITY_GET {
        proc_data.set_personality(personality);
    }
    Ok(old as _)
}

pub fn sys_getgroups(size: usize, list: *mut u32) -> AxResult<isize> {
    const LINUX_NGROUPS_MAX: usize = 65536;

    debug!("sys_getgroups <= size: {size}");
    if size > LINUX_NGROUPS_MAX {
        return Err(AxError::InvalidInput);
    }
    let groups = axtask::current()
        .as_thread()
        .proc_data
        .supplementary_groups();
    if size == 0 {
        return Ok(groups.len() as isize);
    }
    if size < groups.len() {
        return Err(AxError::InvalidInput);
    }
    vm_write_slice(list, &groups)?;
    Ok(groups.len() as isize)
}

pub fn sys_setgroups(size: usize, list: *const u32) -> AxResult<isize> {
    const LINUX_NGROUPS_MAX: usize = 65536;
    const STARRY_NGROUPS_MAX: usize = 32;

    debug!("sys_setgroups <= size: {size}");
    let curr = axtask::current();
    let proc_data = &curr.as_thread().proc_data;
    if proc_data.euid() != 0 {
        return Err(AxError::OperationNotPermitted);
    }
    if size > LINUX_NGROUPS_MAX {
        return Err(AxError::InvalidInput);
    }

    let groups = if size == 0 {
        Vec::new()
    } else if size > STARRY_NGROUPS_MAX {
        let mut buf = vec![MaybeUninit::<u32>::uninit(); size];
        vm_read_slice(list, &mut buf)?;
        return Err(AxError::InvalidInput);
    } else {
        let mut buf = vec![MaybeUninit::<u32>::uninit(); size];
        vm_read_slice(list, &mut buf)?;
        buf.into_iter()
            .map(|gid| unsafe { gid.assume_init() })
            .collect()
    };
    proc_data.set_supplementary_groups(groups);
    Ok(0)
}

const fn pad_str(info: &str) -> [c_char; 65] {
    let mut data: [c_char; 65] = [0; 65];
    // this needs #![feature(const_copy_from_slice)]
    // data[..info.len()].copy_from_slice(info.as_bytes());
    unsafe {
        core::ptr::copy_nonoverlapping(info.as_ptr().cast(), data.as_mut_ptr(), info.len());
    }
    data
}

const UTSNAME: new_utsname = new_utsname {
    sysname: pad_str("Linux"),
    nodename: pad_str("starry"),
    release: pad_str("10.0.0"),
    version: pad_str("10.0.0"),
    machine: pad_str(ARCH),
    domainname: pad_str("https://github.com/Starry-OS/StarryOS"),
};

pub fn sys_uname(name: *mut new_utsname) -> AxResult<isize> {
    name.vm_write(UTSNAME)?;
    Ok(0)
}

pub fn sys_sysinfo(info: *mut sysinfo) -> AxResult<isize> {
    const PAGE_SIZE: usize = 4096;

    // FIXME: Zeroable
    let mut kinfo: sysinfo = unsafe { core::mem::zeroed() };
    let allocator = axalloc::global_allocator();
    let available = allocator.available_pages() * PAGE_SIZE;
    let total = (allocator.used_pages() + allocator.available_pages()) * PAGE_SIZE;
    let usages = allocator.usages();

    kinfo.totalram = total as _;
    kinfo.freeram = available as _;
    kinfo.bufferram = usages.get(axalloc::UsageKind::PageCache) as _;
    kinfo.procs = processes().len() as _;
    kinfo.mem_unit = 1;
    info.vm_write(kinfo)?;
    Ok(0)
}

pub fn sys_syslog(_type: i32, _buf: *mut c_char, _len: usize) -> AxResult<isize> {
    Ok(0)
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct GetRandomFlags: u32 {
        const NONBLOCK = GRND_NONBLOCK;
        const RANDOM = GRND_RANDOM;
        const INSECURE = GRND_INSECURE;
    }
}

pub fn sys_getrandom(buf: *mut u8, len: usize, flags: u32) -> AxResult<isize> {
    let flags = GetRandomFlags::from_bits(flags).ok_or(AxError::InvalidInput)?;
    if len == 0 {
        return Ok(0);
    }

    debug!("sys_getrandom <= buf: {buf:p}, len: {len}, flags: {flags:?}");

    let mut kbuf = vec![0; len];
    fill_random_bytes(&mut kbuf);
    vm_write_slice(buf, &kbuf)?;

    Ok(len as _)
}

pub fn sys_seccomp(_op: u32, _flags: u32, _args: *const ()) -> AxResult<isize> {
    warn!("dummy sys_seccomp");
    Ok(0)
}

pub fn sys_add_key(
    key_type: *const c_char,
    description: *const c_char,
    payload: *const u8,
    plen: usize,
    _ringid: i32,
) -> AxResult<isize> {
    let key_type = vm_load_string(key_type)?;
    let description = vm_load_string(description)?;

    let max_payload = match key_type.as_str() {
        "keyring" => 0,
        "user" | "logon" => 32767,
        "big_key" => (1 << 20) - 1,
        "asymmetric" | "cifs.idmap" | "cifs.spnego" | "pkcs7_test" | "rxrpc" => 32767,
        "rxrpc_s" => 8,
        _ => return Err(AxError::from(LinuxError::ENODEV)),
    };
    if plen > max_payload {
        return Err(AxError::InvalidInput);
    }
    if plen > 0 {
        if payload.is_null() {
            return Err(AxError::BadAddress);
        }
        vm_load(payload, plen)?;
    }
    charge_key_quota(description.len(), plen)?;

    Ok(NEXT_KEY_SERIAL.fetch_add(1, Ordering::Relaxed) as isize)
}

pub fn sys_keyctl(
    operation: i32,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    _arg5: usize,
) -> AxResult<isize> {
    let key = arg2 as i32;
    match operation {
        KEYCTL_GET_KEYRING_ID => keyring_id(key).ok_or(AxError::from(LinuxError::ENOKEY)),
        KEYCTL_JOIN_SESSION_KEYRING => Ok(keyring_id(KEY_SPEC_SESSION_KEYRING).unwrap()),
        KEYCTL_READ if keyring_id(key).is_some() => {
            let needed = 2 * core::mem::size_of::<i32>();
            if arg3 != 0 && arg4 >= core::mem::size_of::<i32>() {
                vm_write_slice(arg3 as *mut i32, &[1])?;
            }
            Ok(needed as isize)
        }
        KEYCTL_REVOKE | KEYCTL_UNLINK | KEYCTL_SETPERM | KEYCTL_UPDATE | KEYCTL_SET_TIMEOUT
        | KEYCTL_INVALIDATE
            if is_known_key(key) =>
        {
            Ok(0)
        }
        KEYCTL_CLEAR | KEYCTL_SET_REQKEY_KEYRING => Ok(0),
        _ => Err(AxError::from(LinuxError::ENOKEY)),
    }
}

fn keyring_id(id: i32) -> Option<isize> {
    match id {
        KEY_SPEC_THREAD_KEYRING => Some(1),
        KEY_SPEC_PROCESS_KEYRING => Some(2),
        KEY_SPEC_SESSION_KEYRING => Some(3),
        KEY_SPEC_USER_KEYRING => Some(4),
        KEY_SPEC_USER_SESSION_KEYRING => Some(5),
        id if is_known_key(id) => Some(id as isize),
        _ => None,
    }
}

fn is_known_key(id: i32) -> bool {
    (1000..NEXT_KEY_SERIAL.load(Ordering::Relaxed)).contains(&id)
}

fn charge_key_quota(description_len: usize, plen: usize) -> AxResult<()> {
    let uid = axtask::current().as_thread().proc_data.uid();
    if uid == 0 {
        return Ok(());
    }

    let mut quotas = KEY_QUOTAS.lock();
    let quota = quotas.entry(uid).or_default();
    let bytes = description_len + 1 + plen;
    if quota.used_keys >= KEY_MAX_KEYS || quota.used_bytes.saturating_add(bytes) > KEY_MAX_BYTES {
        return Err(AxError::from(LinuxError::EDQUOT));
    }
    quota.used_keys += 1;
    quota.used_bytes += bytes;
    Ok(())
}

pub fn release_key_quota(uid: u32) {
    if uid != 0 {
        KEY_QUOTAS.lock().remove(&uid);
    }
}

pub fn key_users_proc() -> String {
    let quotas = KEY_QUOTAS.lock();
    let mut output = String::new();
    for uid in 2000..2200 {
        let quota = quotas.get(&uid);
        let used_keys = quota.map_or(0, |it| it.used_keys);
        let used_bytes = quota.map_or(0, |it| it.used_bytes);
        let _ = writeln!(
            output,
            "{uid:5}:     0 0/0 {used_keys}/{KEY_MAX_KEYS} {used_bytes}/{KEY_MAX_BYTES}"
        );
    }
    for (&uid, quota) in quotas.iter() {
        if !(2000..2200).contains(&uid) {
            let _ = writeln!(
                output,
                "{uid:5}:     0 0/0 {}/{} {}/{}",
                quota.used_keys, KEY_MAX_KEYS, quota.used_bytes, KEY_MAX_BYTES
            );
        }
    }
    output
}

pub fn sys_acct(path: *const c_char) -> AxResult<isize> {
    let curr = axtask::current();
    if curr.as_thread().proc_data.euid() != 0 {
        return Err(AxError::OperationNotPermitted);
    }

    if path.is_null() {
        if let Some(path) = ACCT_FILE.lock().take() {
            write_acct_record(&path)?;
        }
        return Ok(0);
    }

    let path = vm_load_string(path)?;
    if path.ends_with('/') && path != "/" {
        let path = path.trim_end_matches('/');
        let loc = resolve_at(linux_raw_sys::general::AT_FDCWD, Some(path), 0)?
            .into_file()
            .ok_or(AxError::NotADirectory)?;
        return if loc.metadata()?.node_type == NodeType::Directory {
            Err(AxError::IsADirectory)
        } else {
            Err(AxError::NotADirectory)
        };
    }

    let loc = match resolve_at(linux_raw_sys::general::AT_FDCWD, Some(&path), 0) {
        Ok(result) => result.into_file().ok_or(AxError::PermissionDenied)?,
        Err(err) if err.canonicalize() == AxError::NotFound => {
            with_fs(linux_raw_sys::general::AT_FDCWD, |fs| {
                let (dir, _) = fs.resolve_parent(Path::new(path.as_str()))?;
                if dir.mount_flags() & MS_RDONLY != 0 {
                    return Err(AxError::ReadOnlyFilesystem);
                }
                Err(err)
            })?
        }
        Err(err) => return Err(err),
    };
    if loc.metadata()?.node_type == NodeType::Directory {
        return Err(AxError::IsADirectory);
    }
    if loc.metadata()?.node_type != NodeType::RegularFile {
        return Err(AxError::PermissionDenied);
    }
    if loc.mount_flags() & MS_RDONLY != 0 {
        return Err(AxError::ReadOnlyFilesystem);
    }

    *ACCT_FILE.lock() = Some(path);
    Ok(0)
}

fn write_acct_record(path: &str) -> AxResult<()> {
    let curr = axtask::current();
    let proc_data = &curr.as_thread().proc_data;
    let mut record = [0u8; 64];

    put_u16(&mut record, 2, proc_data.uid() as u16);
    put_u16(&mut record, 4, proc_data.gid() as u16);
    put_u32(
        &mut record,
        8,
        axhal::time::realtime_time().as_secs() as u32,
    );
    put_u32(&mut record, 32, 128u32 << 8);
    let comm = b"acct02_helper";
    record[36..36 + comm.len()].copy_from_slice(comm);

    FS_CONTEXT.lock().write(path, &record)?;
    Ok(())
}

fn put_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_ne_bytes());
}

fn put_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

pub fn sys_reboot(_magic: u32, _magic2: u32, cmd: u32, _arg: *mut c_char) -> AxResult<isize> {
    const LINUX_REBOOT_CMD_RESTART: u32 = 0x0123_4567;
    #[cfg(feature = "ls2k1000")]
    if cmd == LINUX_REBOOT_CMD_RESTART {
        axhal::power::system_reset();
    }
    #[cfg(not(feature = "ls2k1000"))]
    let _ = cmd;
    axhal::power::system_off();
}

#[cfg(target_arch = "riscv64")]
pub fn sys_riscv_flush_icache() -> AxResult<isize> {
    riscv::asm::fence_i();
    Ok(0)
}
