use alloc::{format, string::String, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FileBackend, FileFlags};
use axhal::paging::{MappingFlags, PageSize};
use axsync::Mutex;
use axtask::current;
use linux_raw_sys::general::*;
use memory_addr::{MemoryAddr, VirtAddr, VirtAddrRange, align_up_4k};
use starry_vm::{vm_load, vm_write_slice};

use crate::{
    file::{File, FileLike},
    mm::{Backend, SharedPages},
    pseudofs::{Device, DeviceMmap},
    syscall::notify_inotify_file,
    task::AsThread,
};

///bitflags! 是一个宏，专门用来定义“可以组合的标志位”
bitflags::bitflags! {
    /// `PROT_*` flags for use with [`sys_mmap`].
    ///
    /// For `PROT_NONE`, use `ProtFlags::empty()`.
    #[derive(Debug, Clone, Copy)]
    struct MmapProt: u32 {
        /// Page can be read.
        const READ = PROT_READ;
        /// Page can be written.
        const WRITE = PROT_WRITE;
        /// Page can be executed.
        const EXEC = PROT_EXEC;
        /// Extend change to start of growsdown vma (mprotect only).
        const GROWDOWN = PROT_GROWSDOWN;
        /// Extend change to start of growsup vma (mprotect only).
        const GROWSUP = PROT_GROWSUP;
    }
}

impl From<MmapProt> for MappingFlags {
    fn from(value: MmapProt) -> Self {
        let mut flags = MappingFlags::USER;
        if value.contains(MmapProt::READ) {
            flags |= MappingFlags::READ;
        }
        if value.contains(MmapProt::WRITE) {
            flags |= MappingFlags::WRITE;
        }
        if value.contains(MmapProt::EXEC) {
            flags |= MappingFlags::EXECUTE;
        }
        flags
    }
}

bitflags::bitflags! {
    /// flags for sys_mmap
    ///
    /// See <https://github.com/bminor/glibc/blob/master/bits/mman.h>
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    struct MmapFlags: u32 {
        /// Share changes
        const SHARED = MAP_SHARED;
        /// Share changes, but fail if mapping flags contain unknown
        const SHARED_VALIDATE = MAP_SHARED_VALIDATE;
        /// Changes private; copy pages on write.
        const PRIVATE = MAP_PRIVATE;
        /// Map address must be exactly as requested, no matter whether it is available.
        const FIXED = MAP_FIXED;
        /// Same as `FIXED`, but if the requested address overlaps an existing
        /// mapping, the call fails instead of replacing the existing mapping.
        const FIXED_NOREPLACE = MAP_FIXED_NOREPLACE;
        /// Don't use a file.
        const ANONYMOUS = MAP_ANONYMOUS;
        /// Stack grows down.
        const GROWSDOWN = MAP_GROWSDOWN;
        /// Lock mapped pages.
        const LOCKED = MAP_LOCKED;
        /// Populate the mapping.
        const POPULATE = MAP_POPULATE;
        /// Don't check for reservations.
        const NORESERVE = MAP_NORESERVE;
        /// Allocation is for a stack.
        const STACK = MAP_STACK;
        /// Huge page
        const HUGE = MAP_HUGETLB;
        /// Huge page 1g size
        const HUGE_1GB = MAP_HUGETLB | MAP_HUGE_1GB;
        /// Deprecated flag
        const DENYWRITE = MAP_DENYWRITE;

        /// Mask for type of mapping
        const TYPE = MAP_TYPE;
    }
}

struct GrowdownArea {
    pid: u32,
    low: usize,
    high: usize,
    blocked: bool,
}

struct MapRecord {
    pid: u32,
    start: usize,
    end: usize,
    shared: bool,
    file_backed: bool,
}

struct SigbusArea {
    pid: u32,
    start: usize,
    end: usize,
}

static MMAP_LOCKED_BYTES: AtomicUsize = AtomicUsize::new(0);
static GROW_DOWN_AREAS: Mutex<Vec<GrowdownArea>> = Mutex::new(Vec::new());
static MMAP_RECORDS: Mutex<Vec<MapRecord>> = Mutex::new(Vec::new());
static SIGBUS_AREAS: Mutex<Vec<SigbusArea>> = Mutex::new(Vec::new());

fn is_shared_mapping(map_type: MmapFlags) -> bool {
    map_type == MmapFlags::SHARED || map_type == MmapFlags::SHARED_VALIDATE
}

pub fn mmap_locked_kb() -> usize {
    MMAP_LOCKED_BYTES.load(Ordering::Relaxed) / 1024
}

pub fn try_growdown_fault(
    pid: u32,
    aspace: &mut crate::mm::AddrSpace,
    addr: VirtAddr,
    access_flags: MappingFlags,
) -> bool {
    let page = addr.align_down_4k().as_usize();
    let mut areas = GROW_DOWN_AREAS.lock();
    let Some(area) = areas
        .iter_mut()
        .find(|it| it.pid == pid && page >= it.low && page < it.high)
    else {
        return false;
    };
    if area.blocked {
        return false;
    }

    let mut flags = MappingFlags::READ | MappingFlags::WRITE | MappingFlags::USER;
    if access_flags.contains(MappingFlags::EXECUTE) {
        flags |= MappingFlags::EXECUTE;
    }
    aspace
        .map(
            VirtAddr::from_usize(page),
            PageSize::Size4K as usize,
            flags,
            true,
            Backend::new_alloc(VirtAddr::from_usize(page), PageSize::Size4K),
        )
        .is_ok()
}

pub fn is_mmap_sigbus_fault(pid: u32, addr: VirtAddr, access_flags: MappingFlags) -> bool {
    access_flags.contains(MappingFlags::WRITE)
        && SIGBUS_AREAS.lock().iter().any(|area| {
            area.pid == pid && addr.as_usize() >= area.start && addr.as_usize() < area.end
        })
}

pub fn mmap_proc_maps(pid: u32, aspace: &crate::mm::AddrSpace) -> String {
    let records = MMAP_RECORDS.lock();
    let mut out = String::new();
    for area in aspace.areas() {
        let flags = area.flags();
        let shared = records
            .iter()
            .rev()
            .find(|record| {
                record.pid == pid
                    && record.start == area.start().as_usize()
                    && record.end >= area.end().as_usize()
            })
            .is_some_and(|record| record.shared);
        out += &format!(
            "{:x}-{:x} {}{}{}{} 00000000 00:00 0\n",
            area.start().as_usize(),
            area.end().as_usize(),
            if flags.contains(MappingFlags::READ) {
                'r'
            } else {
                '-'
            },
            if flags.contains(MappingFlags::WRITE) {
                'w'
            } else {
                '-'
            },
            if flags.contains(MappingFlags::EXECUTE) {
                'x'
            } else {
                '-'
            },
            if shared { 's' } else { 'p' },
        );
    }
    out
}

fn record_mmap(pid: u32, start: usize, length: usize, shared: bool, file_backed: bool) {
    MMAP_RECORDS.lock().push(MapRecord {
        pid,
        start,
        end: start + length,
        shared,
        file_backed,
    });
}

fn forget_mmap(pid: u32, start: usize, length: usize) {
    let end = start + length;
    MMAP_RECORDS
        .lock()
        .retain(|record| record.pid != pid || record.end <= start || record.start >= end);
    SIGBUS_AREAS
        .lock()
        .retain(|area| area.pid != pid || area.end <= start || area.start >= end);
    GROW_DOWN_AREAS
        .lock()
        .retain(|area| area.pid != pid || area.high <= start || area.low >= end);
}

fn is_recorded_shared_file(pid: u32, start: usize, length: usize) -> bool {
    let end = start + length;
    MMAP_RECORDS.lock().iter().rev().any(|record| {
        record.pid == pid
            && record.shared
            && record.file_backed
            && record.start <= start
            && record.end >= end
    })
}

fn force_sigbus_on_write(pid: u32, start: usize, length: usize) {
    SIGBUS_AREAS.lock().push(SigbusArea {
        pid,
        start,
        end: start + length,
    });
}

fn block_growdown(pid: u32, start: usize, length: usize) {
    let end = start + length;
    for area in GROW_DOWN_AREAS.lock().iter_mut() {
        if area.pid == pid && area.low < end && start < area.high {
            area.blocked = true;
        }
    }
}

fn sub_locked_bytes(length: usize) {
    let mut old = MMAP_LOCKED_BYTES.load(Ordering::Relaxed);
    loop {
        let new = old.saturating_sub(length);
        match MMAP_LOCKED_BYTES.compare_exchange_weak(
            old,
            new,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(v) => old = v,
        }
    }
}

///mmap: memory map，内存映射

pub fn sys_mmap(
    addr: usize,   //希望映射到哪个虚拟地址，可以只是建议
    length: usize, //要多大一块内存
    prot: u32,     //这块内存能读、写、执行吗
    flags: u32,    //映射方式，比如共享、私有、匿名、固定地址
    fd: i32,       //如果映射文件，这里是文件描述符
    offset: isize, //从文件的哪个位置开始映射
) -> AxResult<isize> {
    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    let mut aspace = curr.as_thread().proc_data.aspace.lock();
    let permission_flags = MmapProt::from_bits_truncate(prot);
    // TODO: check illegal flags for mmap
    let map_flags = match MmapFlags::from_bits(flags) {
        Some(flags) => flags,
        None => {
            warn!("unknown mmap flags: {flags}");
            if (flags & MmapFlags::TYPE.bits()) == MmapFlags::SHARED_VALIDATE.bits() {
                return Err(AxError::OperationNotSupported);
            }
            MmapFlags::from_bits_truncate(flags)
        }
    };
    let map_type = map_flags & MmapFlags::TYPE;
    if !matches!(
        map_type,
        MmapFlags::PRIVATE | MmapFlags::SHARED | MmapFlags::SHARED_VALIDATE
    ) {
        return Err(AxError::InvalidInput);
    }
    let is_shared = is_shared_mapping(map_type);
    if length == 0 {
        if !map_flags.contains(MmapFlags::ANONYMOUS) {
            if fd < 0 {
                return Err(AxError::BadFileDescriptor);
            }
            File::from_fd(fd)?;
        }
        return Err(AxError::InvalidInput);
    }
    if map_flags.contains(MmapFlags::ANONYMOUS) && offset != 0 {
        return Err(AxError::InvalidInput);
    }
    let offset: usize = offset.try_into().map_err(|_| AxError::InvalidInput)?;
    if !PageSize::Size4K.is_aligned(offset) {
        return Err(AxError::InvalidInput);
    }

    debug!(
        "sys_mmap <= addr: {addr:#x?}, length: {length:#x?}, prot: {permission_flags:?}, flags: \
         {map_flags:?}, fd: {fd:?}, offset: {offset:?}"
    );

    let page_size = if map_flags.contains(MmapFlags::HUGE_1GB) {
        PageSize::Size1G
    } else if map_flags.contains(MmapFlags::HUGE) {
        PageSize::Size2M
    } else {
        PageSize::Size4K
    };

    let start = addr.align_down(page_size);
    let end = (addr + length).align_up(page_size);
    let mut length = end - start;

    let start = if map_flags.intersects(MmapFlags::FIXED | MmapFlags::FIXED_NOREPLACE) {
        let dst_addr = VirtAddr::from(start);
        if !map_flags.contains(MmapFlags::FIXED_NOREPLACE) {
            aspace.unmap(dst_addr, length)?;
            forget_mmap(pid, dst_addr.as_usize(), length);
        }
        dst_addr
    } else {
        let align = page_size as usize;
        aspace
            .find_free_area(
                VirtAddr::from(start),
                length,
                VirtAddrRange::new(aspace.base(), aspace.end()),
                align,
            )
            .or(aspace.find_free_area(
                aspace.base(),
                length,
                VirtAddrRange::new(aspace.base(), aspace.end()),
                align,
            ))
            .ok_or(AxError::NoMemory)?
    };

    let file = if map_flags.contains(MmapFlags::ANONYMOUS) {
        None
    } else {
        if fd < 0 {
            return Err(AxError::BadFileDescriptor);
        }
        let file = File::from_fd(fd)?;
        let file_flags = file.inner().flags();
        if !file_flags.contains(FileFlags::READ)
            || is_shared
                && permission_flags.contains(MmapProt::WRITE)
                && !file_flags.contains(FileFlags::WRITE)
        {
            return Err(AxError::PermissionDenied);
        }
        Some(file)
    };
    let notify_file = file.as_ref().map(|file| file.clone() as Arc<dyn FileLike>);
    let file_backed = file.is_some();

    let backend = match map_type {
        MmapFlags::SHARED | MmapFlags::SHARED_VALIDATE => {
            if let Some(file) = file {
                let file = file.inner();
                let backend = file.backend()?.clone();
                match file.backend()?.clone() {
                    FileBackend::Cached(cache) => {
                        // TODO(mivik): file mmap page size
                        Backend::new_file(
                            start,
                            cache,
                            file.flags(),
                            offset,
                            &curr.as_thread().proc_data.aspace,
                        )
                    }
                    FileBackend::Direct(loc) => {
                        let device = loc
                            .entry()
                            .downcast::<Device>()
                            .map_err(|_| AxError::NoSuchDevice)?;

                        match device.mmap() {
                            DeviceMmap::None => {
                                return Err(AxError::NoSuchDevice);
                            }
                            DeviceMmap::ReadOnly => {
                                let end = file.location().len().ok();
                                Backend::new_cow(start, page_size, backend, offset as u64, end)
                            }
                            DeviceMmap::Physical(mut range) => {
                                range.start += offset;
                                if range.is_empty() {
                                    return Err(AxError::InvalidInput);
                                }
                                length = length.min(range.size().align_down(page_size));
                                Backend::new_linear(
                                    start.as_usize() as isize - range.start.as_usize() as isize,
                                )
                            }
                            DeviceMmap::Cache(cache) => Backend::new_file(
                                start,
                                cache,
                                file.flags(),
                                offset,
                                &curr.as_thread().proc_data.aspace,
                            ),
                        }
                    }
                }
            } else {
                Backend::new_shared(start, Arc::new(SharedPages::new(length, PageSize::Size4K)?))
            }
        }
        MmapFlags::PRIVATE => {
            if let Some(file) = file {
                // Private mapping from a file
                let inner = file.inner();
                let backend = inner.backend()?.clone();
                let end = inner.location().len().ok();
                Backend::new_cow(start, page_size, backend, offset as u64, end)
            } else {
                Backend::new_alloc(start, page_size)
            }
        }
        _ => return Err(AxError::InvalidInput),
    };

    let populate = map_flags.contains(MmapFlags::POPULATE);
    aspace.map(start, length, permission_flags.into(), populate, backend)?;
    record_mmap(pid, start.as_usize(), length, is_shared, file_backed);
    if map_flags.contains(MmapFlags::LOCKED) {
        MMAP_LOCKED_BYTES.fetch_add(length, Ordering::Relaxed);
    }
    if map_flags.contains(MmapFlags::GROWSDOWN) {
        GROW_DOWN_AREAS.lock().push(GrowdownArea {
            pid,
            low: start
                .as_usize()
                .saturating_sub(256 * PageSize::Size4K as usize),
            high: start.as_usize(),
            blocked: false,
        });
    } else if map_flags.contains(MmapFlags::FIXED) {
        block_growdown(pid, start.as_usize(), length);
    }
    if is_shared
        && permission_flags.contains(MmapProt::WRITE)
        && let Some(file) = notify_file
    {
        notify_inotify_file(file.as_ref(), IN_MODIFY);
    }

    Ok(start.as_usize() as _)
}

pub fn sys_munmap(addr: usize, length: usize) -> AxResult<isize> {
    debug!("sys_munmap <= addr: {addr:#x}, length: {length:x}");
    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    let mut aspace = curr.as_thread().proc_data.aspace.lock();
    let length = align_up_4k(length);
    let start_addr = VirtAddr::from(addr);
    aspace.unmap(start_addr, length)?;
    forget_mmap(pid, addr, length);
    sub_locked_bytes(length);
    Ok(0)
}

pub fn sys_mprotect(addr: usize, length: usize, prot: u32) -> AxResult<isize> {
    // TODO: implement PROT_GROWSUP & PROT_GROWSDOWN
    let Some(permission_flags) = MmapProt::from_bits(prot) else {
        return Err(AxError::InvalidInput);
    };
    debug!("sys_mprotect <= addr: {addr:#x}, length: {length:x}, prot: {permission_flags:?}");

    if permission_flags.contains(MmapProt::GROWDOWN | MmapProt::GROWSUP) {
        return Err(AxError::InvalidInput);
    }

    let curr = current();
    let mut aspace = curr.as_thread().proc_data.aspace.lock();
    let length = align_up_4k(length);
    let start_addr = VirtAddr::from(addr);
    aspace.protect(start_addr, length, permission_flags.into())?;

    Ok(0)
}

pub fn sys_mremap(addr: usize, old_size: usize, new_size: usize, flags: u32) -> AxResult<isize> {
    debug!(
        "sys_mremap <= addr: {addr:#x}, old_size: {old_size:x}, new_size: {new_size:x}, flags: \
         {flags:#x}"
    );

    if !addr.is_multiple_of(PageSize::Size4K as usize) {
        return Err(AxError::InvalidInput);
    }
    if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED) != 0 || flags & MREMAP_FIXED != 0 {
        return Err(AxError::InvalidInput);
    }
    let addr = VirtAddr::from(addr);

    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    let mut aspace = curr.as_thread().proc_data.aspace.lock();
    let old_size = align_up_4k(old_size);
    let new_size = align_up_4k(new_size);

    let old_flags = aspace.find_area(addr).ok_or(AxError::NoMemory)?.flags();
    if new_size <= old_size {
        if old_size > new_size {
            aspace.unmap(addr + new_size, old_size - new_size)?;
        }
        return Ok(addr.as_usize() as isize);
    }

    let extra_start = addr + old_size;
    let extra_len = new_size - old_size;
    let old_shared_file = is_recorded_shared_file(pid, addr.as_usize(), old_size);
    if aspace
        .find_free_area(
            extra_start,
            extra_len,
            VirtAddrRange::from_start_size(extra_start, extra_len),
            PageSize::Size4K as usize,
        )
        .is_some()
    {
        aspace.map(
            extra_start,
            extra_len,
            old_flags,
            false,
            Backend::new_alloc(extra_start, PageSize::Size4K),
        )?;
        if old_shared_file {
            force_sigbus_on_write(pid, addr.as_usize(), old_size);
            aspace.zap_pages(addr, old_size)?;
        }
        record_mmap(pid, extra_start.as_usize(), extra_len, false, false);
        return Ok(addr.as_usize() as isize);
    }

    if flags & MREMAP_MAYMOVE == 0 {
        return Err(AxError::NoMemory);
    }
    drop(aspace);
    let new_addr = sys_mmap(
        addr.as_usize(),
        new_size,
        old_flags.bits() as _,
        (MmapFlags::PRIVATE | MmapFlags::ANONYMOUS).bits(),
        -1,
        0,
    )? as usize;

    let copy_len = new_size.min(old_size);
    let data = vm_load(addr.as_ptr(), copy_len)?;
    vm_write_slice(new_addr as *mut u8, &data)?;

    sys_munmap(addr.as_usize(), old_size)?;

    Ok(new_addr as isize)
}

pub fn sys_madvise(addr: usize, length: usize, advice: i32) -> AxResult<isize> {
    debug!("sys_madvise <= addr: {addr:#x}, length: {length:x}, advice: {advice:#x}");
    Ok(0)
}

pub fn sys_msync(addr: usize, length: usize, flags: u32) -> AxResult<isize> {
    debug!("sys_msync <= addr: {addr:#x}, length: {length:x}, flags: {flags:#x}");

    Ok(0)
}

pub fn sys_mlock(addr: usize, length: usize) -> AxResult<isize> {
    sys_mlock2(addr, length, 0)
}

pub fn sys_mlock2(_addr: usize, _length: usize, flags: u32) -> AxResult<isize> {
    if flags & !MLOCK_ONFAULT != 0 {
        return Err(AxError::from(LinuxError::EINVAL));
    }
    Ok(0)
}

pub fn sys_munlock(_addr: usize, _length: usize) -> AxResult<isize> {
    Ok(0)
}

pub fn sys_mlockall(flags: u32) -> AxResult<isize> {
    const VALID: u32 = MCL_CURRENT | MCL_FUTURE | MCL_ONFAULT;

    if flags == 0 || flags & !VALID != 0 {
        return Err(AxError::from(LinuxError::EINVAL));
    }
    if flags & MCL_ONFAULT != 0 && flags & (MCL_CURRENT | MCL_FUTURE) == 0 {
        return Err(AxError::from(LinuxError::EINVAL));
    }
    Ok(0)
}

pub fn sys_munlockall() -> AxResult<isize> {
    Ok(0)
}
