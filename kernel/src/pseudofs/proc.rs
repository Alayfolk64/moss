use alloc::{
    borrow::Cow,
    boxed::Box,
    format,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::{
    any::Any,
    ffi::CStr,
    iter,
    mem::size_of,
    sync::atomic::{AtomicUsize, Ordering},
    task::Context,
};

use axfs::drop_global_file_caches;
use axfs_ng_vfs::{
    DeviceId, FileNodeOps, Filesystem, FilesystemOps, Metadata, MetadataUpdate, NodeFlags, NodeOps,
    NodePermission, NodeType, VfsError, VfsResult,
};
use axhal::paging::MappingFlags;
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
use axtask::{AxTaskRef, WeakAxTaskRef, current};
use indoc::indoc;
use linux_raw_sys::{
    general::{CLOCK_BOOTTIME, CLOCK_MONOTONIC, CLONE_NEWPID, CLONE_NEWUSER, CLONE_NEWUTS},
    ioctl::{NS_GET_NSTYPE, NS_GET_OWNER_UID, NS_GET_PARENT, NS_GET_USERNS},
};
use memory_addr::{MemoryAddr, VirtAddr};
use starry_process::{Process, init_proc};
use starry_vm::VmMutPtr;

use crate::{
    file::{FD_TABLE, FileLike, Kstat, add_file_like, pipe_max_size, set_pipe_max_size},
    pseudofs::{
        DirMaker, DirMapping, NodeOpsMux, RwFile, SimpleDir, SimpleDirOps, SimpleFile,
        SimpleFileOperation, SimpleFs, SimpleFsNode, StaticFile,
    },
    task::{AsThread, NamespaceState, TaskStat, get_task, tasks},
};

const DUMMY_SCHED_DEBUG: &str = indoc! {"
    Sched Debug Version: v0.11
    sysctl_sched_latency                    : 12000000
    sysctl_sched_min_granularity            : 1500000
    sysctl_sched_wakeup_granularity         : 2000000
    sysctl_sched_child_runs_first           : 0
    .nr_running                    : 1
    .load                          : 0
"};

const DUMMY_CPUINFO: &str = indoc! {"
    processor	: 0
    model name	: QEMU Virtual CPU
    cpu MHz		: 1000.000
    bogomips	: 1000.00
"};

fn proc_stat() -> String {
    let mut stat = String::from("cpu  0 0 0 0 0 0 0 0 0 0\n");
    for cpu_id in 0..axhal::cpu_num() {
        stat.push_str(&format!("cpu{cpu_id} 0 0 0 0 0 0 0 0 0 0\n"));
    }
    stat.push_str("intr 0\nctxt 0\nbtime 0\nprocesses 0\nprocs_running 1\nprocs_blocked 0\n");
    stat
}

fn proc_uptime() -> String {
    let uptime = axhal::time::monotonic_time();
    format!(
        "{}.{:02} 0.00\n",
        uptime.as_secs(),
        uptime.subsec_millis() / 10
    )
}

#[cfg(feature = "net")]
fn proc_net_dev() -> String {
    let mut output =
        String::from(
            "Inter-|   Receive                                                |  Transmit\nface \
             |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs \
             drop fifo colls carrier compressed\nlo:       0       0    0    0    0     0          \
             0         0        0       0    0    0    0     0       0          0\n",
        );
    if axnet::eth0_exists() {
        output.push_str(
            "  eth0:       0       0    0    0    0     0          0         0        0       0    \
             0    0    0     0       0          0\n",
        );
    }
    output
}

#[cfg(feature = "net")]
fn proc_net_route() -> String {
    let mut output = String::from(
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
    );
    let Some(config) = axnet::ipv4_config() else {
        return output;
    };
    let mask = if config.prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(config.prefix_len))
    };
    let network = u32::from(config.address) & mask;
    let route_hex = |value: u32| u32::from_le_bytes(value.to_be_bytes());
    output.push_str(&format!(
        "eth0\t{:08X}\t00000000\t0001\t0\t0\t0\t{:08X}\t0\t0\t0\n",
        route_hex(network),
        route_hex(mask),
    ));
    if let Some(gateway) = config.gateway {
        output.push_str(&format!(
            "eth0\t00000000\t{:08X}\t0003\t0\t0\t0\t00000000\t0\t0\t0\n",
            u32::from_le_bytes(gateway.octets()),
        ));
    }
    output
}

pub fn new_procfs() -> Filesystem {
    SimpleFs::new_with("proc".into(), 0x9fa0, builder)
}

struct ProcessTaskDir {
    fs: Arc<SimpleFs>,
    process: Weak<Process>,
}

impl SimpleDirOps for ProcessTaskDir {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        let Some(process) = self.process.upgrade() else {
            return Box::new(iter::empty());
        };
        Box::new(
            process
                .threads()
                .into_iter()
                .map(|tid| tid.to_string().into()),
        )
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let process = self.process.upgrade().ok_or(VfsError::NotFound)?;
        let tid = name.parse::<u32>().map_err(|_| VfsError::NotFound)?;
        let task = get_task(tid).map_err(|_| VfsError::NotFound)?;
        if task.as_thread().proc_data.proc.pid() != process.pid() {
            return Err(VfsError::NotFound);
        }

        Ok(NodeOpsMux::Dir(SimpleDir::new_maker(
            self.fs.clone(),
            Arc::new(ThreadDir {
                fs: self.fs.clone(),
                task: Arc::downgrade(&task),
            }),
        )))
    }

    fn is_cacheable(&self) -> bool {
        false
    }
}

#[rustfmt::skip]
fn task_status(task: &AxTaskRef) -> String {
    let proc_data = &task.as_thread().proc_data;
    let rss_kb = proc_data.current_rss_kb();
    proc_data.proc.record_maxrss_kb(rss_kb);
    let hwm_kb = proc_data.proc.maxrss_kb();
    format!(
        "Tgid:\t{}\n\
        Pid:\t{}\n\
        TracerPid:\t{}\n\
        Uid:\t0 0 0 0\n\
        Gid:\t0 0 0 0\n\
        VmHWM:\t{} kB\n\
        VmRSS:\t{} kB\n\
        VmSwap:\t0 kB\n\
        VmLck:\t{} kB\n\
        Cpus_allowed:\t1\n\
        Cpus_allowed_list:\t0\n\
        Mems_allowed:\t1\n\
        Mems_allowed_list:\t0",
        proc_data.proc.pid(),
        task.id().as_u64(),
        crate::syscall::ptrace_tracer_pid(proc_data.proc.pid()).unwrap_or(0),
        hwm_kb,
        rss_kb,
        crate::syscall::mmap_locked_kb(),
    )
}

#[rustfmt::skip]
fn process_status(process: &Process) -> String {
    format!(
        "Tgid:\t{}\n\
        Pid:\t{}\n\
        TracerPid:\t0\n\
        Uid:\t0 0 0 0\n\
        Gid:\t0 0 0 0\n\
        VmHWM:\t{} kB\n\
        VmRSS:\t0 kB\n\
        VmSwap:\t0 kB\n\
        VmLck:\t{} kB\n\
        Cpus_allowed:\t1\n\
        Cpus_allowed_list:\t0\n\
        Mems_allowed:\t1\n\
        Mems_allowed_list:\t0",
        process.pid(),
        process.pid(),
        process.maxrss_kb(),
        crate::syscall::mmap_locked_kb(),
    )
}

fn proc_meminfo() -> String {
    const PAGE_SIZE: usize = 4096;

    let allocator = axalloc::global_allocator();
    let available = allocator.available_pages() * PAGE_SIZE;
    let total = (allocator.used_pages() + allocator.available_pages()) * PAGE_SIZE;
    let usages = allocator.usages();
    let cached = usages.get(axalloc::UsageKind::PageCache);

    format!(
        "MemTotal:       {} kB\nMemFree:        {} kB\nMemAvailable:   {} kB\nBuffers:        0 \
         kB\nCached:         {} kB\nSwapCached:     0 kB\nActive:         0 kB\nInactive:       0 \
         kB\nSwapTotal:      0 kB\nSwapFree:       0 kB\n",
        total / 1024,
        available / 1024,
        available / 1024,
        cached / 1024,
    )
}

fn proc_pagemap(fs: Arc<SimpleFs>) -> Arc<SimpleFile> {
    const LEN: usize = 1024 * 1024;
    SimpleFile::new_regular(fs, || {
        let entry = (1u64 << 63).to_ne_bytes();
        let mut data = vec![0; LEN];
        for chunk in data.chunks_exact_mut(entry.len()) {
            chunk.copy_from_slice(&entry);
        }
        Ok(data)
    })
}

struct ProcMemFile {
    node: SimpleFsNode,
    task: WeakAxTaskRef,
}

impl ProcMemFile {
    fn new(fs: Arc<SimpleFs>, task: WeakAxTaskRef) -> Arc<Self> {
        Arc::new(Self {
            node: SimpleFsNode::new(
                fs,
                NodeType::RegularFile,
                NodePermission::from_bits_truncate(0o600),
            ),
            task,
        })
    }

    fn target(&self) -> VfsResult<AxTaskRef> {
        let target = self.task.upgrade().ok_or(VfsError::NotFound)?;
        let caller = current().as_thread().proc_data.proc.pid();
        let target_pid = target.as_thread().proc_data.proc.pid();
        if caller != target_pid && !crate::syscall::ptrace_stopped_by(target_pid, caller) {
            return Err(VfsError::PermissionDenied);
        }
        Ok(target)
    }
}

#[inherit_methods_macro::inherit_methods(from = "self.node")]
impl NodeOps for ProcMemFile {
    fn inode(&self) -> u64;
    fn metadata(&self) -> VfsResult<Metadata>;
    fn update_metadata(&self, update: MetadataUpdate) -> VfsResult<()>;
    fn filesystem(&self) -> &dyn FilesystemOps;
    fn sync(&self, data_only: bool) -> VfsResult<()>;

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn len(&self) -> VfsResult<u64> {
        Ok(0)
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE
    }
}

impl FileNodeOps for ProcMemFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let address = usize::try_from(offset).map_err(|_| VfsError::BadAddress)?;
        let target = self.target()?;
        let aspace = target.as_thread().proc_data.aspace();
        let mut aspace = aspace.lock();
        // `/proc/pid/mem` follows `pread(2)` partial-I/O semantics: a request
        // that begins in a VMA may return the prefix up to that VMA's end.
        // GDB deliberately reads chunks which can cross an ELF segment gap.
        // Requiring the whole buffer to be mapped makes native GDB lose the
        // dynamic link map (notably for LoongArch PIE executables).
        let start = VirtAddr::from_usize(address);
        let area = aspace.find_area(start).ok_or(VfsError::BadAddress)?;
        let len = buf.len().min(area.end() - start);
        let end = address.checked_add(len).ok_or(VfsError::BadAddress)?;
        let start_page = VirtAddr::from_usize(address).align_down_4k();
        let end_page = VirtAddr::from_usize(end).align_up_4k();
        aspace.populate_area(start_page, end_page - start_page, MappingFlags::READ)?;
        aspace.read(start, &mut buf[..len])?;
        Ok(len)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> VfsResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let address = usize::try_from(offset).map_err(|_| VfsError::BadAddress)?;
        let target = self.target()?;
        let target_pid = target.as_thread().proc_data.proc.pid();
        let aspace = target.as_thread().proc_data.aspace();
        let mut aspace = aspace.lock();
        let start = VirtAddr::from_usize(address);
        let area = aspace.find_area(start).ok_or(VfsError::BadAddress)?;
        let len = buf.len().min(area.end() - start);
        let end = address.checked_add(len).ok_or(VfsError::BadAddress)?;
        let start_page = VirtAddr::from_usize(address).align_down_4k();
        let end_page = VirtAddr::from_usize(end).align_up_4k();
        // A debugger may write a read-only executable VMA, but the write must
        // first detach a private COW page. Otherwise a software breakpoint in
        // libc would corrupt the shared file-cache page used by the debugger.
        aspace.populate_area(start_page, end_page - start_page, MappingFlags::WRITE)?;
        aspace.write(start, &buf[..len])?;
        drop(aspace);
        crate::syscall::ptrace_note_text_modified(target_pid);
        Ok(len)
    }

    fn append(&self, _buf: &[u8]) -> VfsResult<(usize, u64)> {
        Err(VfsError::InvalidInput)
    }

    fn set_len(&self, _len: u64) -> VfsResult<()> {
        Err(VfsError::InvalidInput)
    }

    fn set_symlink(&self, _target: &str) -> VfsResult<()> {
        Err(VfsError::InvalidInput)
    }
}

impl Pollable for ProcMemFile {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

fn proc_rw_file(fs: Arc<SimpleFs>, initial: impl Into<Vec<u8>>) -> Arc<SimpleFile> {
    let data = Arc::new(Mutex::new(initial.into()));
    SimpleFile::new_regular(
        fs,
        RwFile::new(move |req| match req {
            SimpleFileOperation::Read => Ok(Some(data.lock().clone())),
            SimpleFileOperation::Write(input) => {
                let mut data = data.lock();
                data.clear();
                data.extend_from_slice(input);
                Ok(None::<Vec<u8>>)
            }
        }),
    )
}

fn proc_pipe_max_size_file(fs: Arc<SimpleFs>) -> Arc<SimpleFile> {
    SimpleFile::new_regular(
        fs,
        RwFile::new(move |req| match req {
            SimpleFileOperation::Read => Ok(Some(format!("{}\n", pipe_max_size()).into_bytes())),
            SimpleFileOperation::Write(input) => {
                if input.is_empty() {
                    return Ok(None::<Vec<u8>>);
                }
                let value = str::from_utf8(input)
                    .map_err(|_| VfsError::InvalidInput)?
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| VfsError::InvalidInput)?;
                if value == 0 {
                    return Err(VfsError::InvalidInput);
                }
                set_pipe_max_size(value);
                Ok(None::<Vec<u8>>)
            }
        }),
    )
}

fn proc_timens_offsets_file(fs: Arc<SimpleFs>) -> Arc<SimpleFile> {
    SimpleFile::new_regular(
        fs,
        RwFile::new(move |req| match req {
            SimpleFileOperation::Read => Ok(Some(
                crate::syscall::read_child_timens_offsets().into_bytes(),
            )),
            SimpleFileOperation::Write(input) => {
                let input = str::from_utf8(input).map_err(|_| VfsError::InvalidInput)?;
                let mut parts = input.split_whitespace();
                while let Some(clock) = parts.next() {
                    let secs = parts
                        .next()
                        .ok_or(VfsError::InvalidInput)?
                        .parse()
                        .map_err(|_| VfsError::InvalidInput)?;
                    let nsecs = parts
                        .next()
                        .ok_or(VfsError::InvalidInput)?
                        .parse()
                        .map_err(|_| VfsError::InvalidInput)?;
                    let clock_id = match clock {
                        "monotonic" => CLOCK_MONOTONIC,
                        "boottime" => CLOCK_BOOTTIME,
                        _ => clock.parse().map_err(|_| VfsError::InvalidInput)?,
                    };
                    crate::syscall::set_child_timens_offset(clock_id, secs, nsecs)
                        .map_err(|_| VfsError::InvalidInput)?;
                }
                Ok(None::<Vec<u8>>)
            }
        }),
    )
}

fn proc_bool_sysctl_file(
    fs: Arc<SimpleFs>,
    read: fn() -> bool,
    write: fn(bool),
) -> Arc<SimpleFile> {
    SimpleFile::new_regular(
        fs,
        RwFile::new(move |req| match req {
            SimpleFileOperation::Read => Ok(Some(
                format!("{}\n", if read() { 1 } else { 0 }).into_bytes(),
            )),
            SimpleFileOperation::Write(input) => {
                let input = str::from_utf8(input)
                    .map_err(|_| VfsError::InvalidInput)?
                    .trim();
                match input {
                    "0" => write(false),
                    "1" => write(true),
                    _ => return Err(VfsError::InvalidInput),
                }
                Ok(None::<Vec<u8>>)
            }
        }),
    )
}

fn proc_usize_sysctl_file(
    fs: Arc<SimpleFs>,
    read: fn() -> usize,
    write: fn(usize),
) -> Arc<SimpleFile> {
    SimpleFile::new_regular(
        fs,
        RwFile::new(move |req| match req {
            SimpleFileOperation::Read => Ok(Some(format!("{}\n", read()).into_bytes())),
            SimpleFileOperation::Write(input) => {
                if input.is_empty() {
                    return Ok(None::<Vec<u8>>);
                }
                let input = str::from_utf8(input)
                    .map_err(|_| VfsError::InvalidInput)?
                    .trim();
                if input.is_empty() {
                    return Ok(None::<Vec<u8>>);
                }
                write(input.parse().map_err(|_| VfsError::InvalidInput)?);
                Ok(None::<Vec<u8>>)
            }
        }),
    )
}

fn proc_setgroups_file(fs: Arc<SimpleFs>) -> Arc<SimpleFile> {
    let data = Arc::new(Mutex::new(b"allow\n".to_vec()));
    SimpleFile::new_regular(
        fs,
        RwFile::new(move |req| match req {
            SimpleFileOperation::Read => Ok(Some(data.lock().clone())),
            SimpleFileOperation::Write(input) => {
                let input = str::from_utf8(input).map_err(|_| VfsError::InvalidInput)?;
                let input = input.trim();
                let mut data = data.lock();
                data.clear();
                if input.is_empty() {
                    return Ok(None::<Vec<u8>>);
                }
                if input.starts_with("deny") {
                    data.extend_from_slice(b"deny\n");
                } else if input.starts_with("allow") {
                    data.extend_from_slice(b"allow\n");
                } else {
                    return Err(VfsError::InvalidInput);
                }
                Ok(None::<Vec<u8>>)
            }
        }),
    )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ProcNamespaceKind {
    Pid,
    Uts,
    User,
}

impl ProcNamespaceKind {
    fn name(self) -> &'static str {
        match self {
            Self::Pid => "pid",
            Self::Uts => "uts",
            Self::User => "user",
        }
    }

    fn clone_flag(self) -> u32 {
        match self {
            Self::Pid => CLONE_NEWPID,
            Self::Uts => CLONE_NEWUTS,
            Self::User => CLONE_NEWUSER,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcNamespaceObject {
    kind: ProcNamespaceKind,
    state: NamespaceState,
}

impl ProcNamespaceObject {
    fn new(kind: ProcNamespaceKind, state: NamespaceState) -> Self {
        Self { kind, state }
    }

    fn metadata(self) -> Metadata {
        Metadata {
            device: 0,
            inode: self.state.inode,
            nlink: 1,
            mode: NodePermission::from_bits_truncate(0o444),
            node_type: NodeType::RegularFile,
            uid: 0,
            gid: 0,
            size: 0,
            block_size: 0,
            blocks: 0,
            rdev: DeviceId::default(),
            atime: Default::default(),
            mtime: Default::default(),
            ctime: Default::default(),
        }
    }

    fn stat(self) -> Kstat {
        Kstat {
            dev: 0,
            ino: self.state.inode,
            nlink: 1,
            mode: ((NodeType::RegularFile as u32) << 12) | 0o444,
            uid: 0,
            gid: 0,
            size: 0,
            blksize: 0,
            blocks: 0,
            rdev: DeviceId::default(),
            attributes: 0,
            attributes_mask: 0,
            atime: Default::default(),
            btime: Default::default(),
            mtime: Default::default(),
            ctime: Default::default(),
        }
    }

    fn path(self) -> String {
        format!("{}:[{}]", self.kind.name(), self.state.inode)
    }

    fn parent_state(self) -> Option<NamespaceState> {
        self.state.parent_inode.map(|inode| NamespaceState {
            inode,
            parent_inode: None,
            init_pid: None,
        })
    }

    fn current_inode(self) -> u64 {
        let namespaces = current().as_thread().proc_data.namespaces();
        match self.kind {
            ProcNamespaceKind::Pid => namespaces.pid.inode,
            ProcNamespaceKind::Uts => namespaces.uts.inode,
            ProcNamespaceKind::User => namespaces.user.inode,
        }
    }

    fn open_parent(self) -> VfsResult<usize> {
        let parent = self.parent_state().ok_or(VfsError::OperationNotPermitted)?;
        if self.current_inode() == self.state.inode {
            return Err(VfsError::OperationNotPermitted);
        }
        open_namespace_fd(Self::new(self.kind, parent))
    }

    fn ioctl(self, cmd: u32, arg: usize) -> VfsResult<usize> {
        match cmd {
            NS_GET_NSTYPE => Ok(self.kind.clone_flag() as usize),
            NS_GET_OWNER_UID => {
                if self.kind != ProcNamespaceKind::User {
                    return Err(VfsError::InvalidInput);
                }
                (arg as *mut u32).vm_write(0)?;
                Ok(0)
            }
            NS_GET_PARENT => {
                if self.kind == ProcNamespaceKind::Pid {
                    self.open_parent()
                } else {
                    Err(VfsError::InvalidInput)
                }
            }
            NS_GET_USERNS => {
                if self.kind == ProcNamespaceKind::User {
                    self.open_parent()
                } else {
                    Err(VfsError::OperationNotPermitted)
                }
            }
            _ => Err(VfsError::NotATty),
        }
    }
}

struct ProcNamespaceFd {
    object: ProcNamespaceObject,
}

impl Pollable for ProcNamespaceFd {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

impl FileLike for ProcNamespaceFd {
    fn stat(&self) -> axerrno::AxResult<Kstat> {
        Ok(self.object.stat())
    }

    fn path(&self) -> Cow<'_, str> {
        Cow::Owned(self.object.path())
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> axerrno::AxResult<usize> {
        self.object.ioctl(cmd, arg)
    }
}

fn open_namespace_fd(object: ProcNamespaceObject) -> VfsResult<usize> {
    add_file_like(Arc::new(ProcNamespaceFd { object }), true).map(|fd| fd as usize)
}

struct ProcNamespaceFile {
    fs: Arc<SimpleFs>,
    object: ProcNamespaceObject,
}

impl ProcNamespaceFile {
    fn new(fs: Arc<SimpleFs>, kind: ProcNamespaceKind, state: NamespaceState) -> Arc<Self> {
        Arc::new(Self {
            fs,
            object: ProcNamespaceObject::new(kind, state),
        })
    }
}

impl NodeOps for ProcNamespaceFile {
    fn inode(&self) -> u64 {
        self.object.state.inode
    }

    fn metadata(&self) -> VfsResult<Metadata> {
        Ok(self.object.metadata())
    }

    fn update_metadata(&self, _update: MetadataUpdate) -> VfsResult<()> {
        Ok(())
    }

    fn filesystem(&self) -> &dyn FilesystemOps {
        &*self.fs
    }

    fn len(&self) -> VfsResult<u64> {
        Ok(0)
    }

    fn sync(&self, _data_only: bool) -> VfsResult<()> {
        Ok(())
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE
    }
}

impl FileNodeOps for ProcNamespaceFile {
    fn read_at(&self, _buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        Ok(0)
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> VfsResult<usize> {
        Err(VfsError::BadFileDescriptor)
    }

    fn append(&self, _buf: &[u8]) -> VfsResult<(usize, u64)> {
        Err(VfsError::BadFileDescriptor)
    }

    fn set_len(&self, _len: u64) -> VfsResult<()> {
        Err(VfsError::BadFileDescriptor)
    }

    fn set_symlink(&self, _target: &str) -> VfsResult<()> {
        Err(VfsError::BadFileDescriptor)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> VfsResult<usize> {
        self.object.ioctl(cmd, arg)
    }
}

impl Pollable for ProcNamespaceFile {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

/// The /proc/[pid]/ns directory
struct ThreadNamespaceDir {
    fs: Arc<SimpleFs>,
    task: WeakAxTaskRef,
}

impl SimpleDirOps for ThreadNamespaceDir {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        Box::new(["pid", "uts", "user"].into_iter().map(Cow::Borrowed))
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let task = self.task.upgrade().ok_or(VfsError::NotFound)?;
        let namespaces = task.as_thread().proc_data.namespaces();
        let (kind, state) = match name {
            "pid" => (ProcNamespaceKind::Pid, namespaces.pid),
            "uts" => (ProcNamespaceKind::Uts, namespaces.uts),
            "user" => (ProcNamespaceKind::User, namespaces.user),
            _ => return Err(VfsError::NotFound),
        };
        Ok(ProcNamespaceFile::new(self.fs.clone(), kind, state).into())
    }

    fn is_cacheable(&self) -> bool {
        false
    }
}

/// The /proc/[pid]/fd directory
struct ThreadFdDir {
    fs: Arc<SimpleFs>,
    task: WeakAxTaskRef,
}

impl SimpleDirOps for ThreadFdDir {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        let Some(task) = self.task.upgrade() else {
            return Box::new(iter::empty());
        };
        let ids = FD_TABLE
            .scope(&task.as_thread().proc_data.scope.read())
            .read()
            .ids()
            .map(|id| Cow::Owned(id.to_string()))
            .collect::<Vec<_>>();
        Box::new(ids.into_iter())
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let fs = self.fs.clone();
        let task = self.task.upgrade().ok_or(VfsError::NotFound)?;
        let fd = name.parse::<u32>().map_err(|_| VfsError::NotFound)?;
        let path = FD_TABLE
            .scope(&task.as_thread().proc_data.scope.read())
            .read()
            .get(fd as _)
            .ok_or(VfsError::NotFound)?
            .inner
            .path()
            .into_owned();
        Ok(SimpleFile::new(fs, NodeType::Symlink, move || Ok(path.clone())).into())
    }

    fn is_cacheable(&self) -> bool {
        false
    }
}

/// The /proc/[pid]/fdinfo directory
struct ThreadFdInfoDir {
    fs: Arc<SimpleFs>,
    task: WeakAxTaskRef,
}

impl SimpleDirOps for ThreadFdInfoDir {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        let Some(task) = self.task.upgrade() else {
            return Box::new(iter::empty());
        };
        let ids = FD_TABLE
            .scope(&task.as_thread().proc_data.scope.read())
            .read()
            .ids()
            .map(|id| Cow::Owned(id.to_string()))
            .collect::<Vec<_>>();
        Box::new(ids.into_iter())
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let fs = self.fs.clone();
        let task = self.task.upgrade().ok_or(VfsError::NotFound)?;
        let fd = name.parse::<u32>().map_err(|_| VfsError::NotFound)?;
        let file = FD_TABLE
            .scope(&task.as_thread().proc_data.scope.read())
            .read()
            .get(fd as _)
            .ok_or(VfsError::NotFound)?
            .inner
            .clone();
        Ok(SimpleFile::new_regular(fs, move || {
            Ok(file
                .proc_fdinfo()
                .unwrap_or_else(|| "pos:\t0\nflags:\t0\nmnt_id:\t0\n".to_string()))
        })
        .into())
    }

    fn is_cacheable(&self) -> bool {
        false
    }
}

/// The /proc/[pid] directory
struct ThreadDir {
    fs: Arc<SimpleFs>,
    task: WeakAxTaskRef,
}

impl SimpleDirOps for ThreadDir {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        Box::new(
            [
                "stat",
                "status",
                "oom_score_adj",
                "loginuid",
                "sessionid",
                "task",
                "maps",
                "mem",
                "pagemap",
                "mounts",
                "cmdline",
                "auxv",
                "comm",
                "exe",
                "fd",
                "fdinfo",
                "ns",
                "setgroups",
                "uid_map",
                "gid_map",
                "timens_offsets",
            ]
            .into_iter()
            .map(Cow::Borrowed),
        )
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let fs = self.fs.clone();
        let task = self.task.upgrade().ok_or(VfsError::NotFound)?;
        Ok(match name {
            "stat" => SimpleFile::new_regular(fs, move || {
                Ok(format!("{}", TaskStat::from_thread(&task)?).into_bytes())
            })
            .into(),
            "status" => SimpleFile::new_regular(fs, move || Ok(task_status(&task))).into(),
            "oom_score_adj" => SimpleFile::new_regular(
                fs,
                RwFile::new(move |req| match req {
                    SimpleFileOperation::Read => Ok(Some(
                        task.as_thread().oom_score_adj().to_string().into_bytes(),
                    )),
                    SimpleFileOperation::Write(data) => {
                        if !data.is_empty() {
                            let value = str::from_utf8(data)
                                .ok()
                                .and_then(|it| it.parse::<i32>().ok())
                                .ok_or(VfsError::InvalidInput)?;
                            task.as_thread().set_oom_score_adj(value);
                        }
                        Ok(None)
                    }
                }),
            )
            .into(),
            "loginuid" => SimpleFile::new_regular(
                fs,
                RwFile::new(move |req| match req {
                    SimpleFileOperation::Read => Ok(Some(
                        format!("{}\n", task.as_thread().proc_data.loginuid()).into_bytes(),
                    )),
                    SimpleFileOperation::Write(data) => {
                        if current().as_thread().proc_data.euid() != 0 {
                            return Err(VfsError::PermissionDenied);
                        }
                        let input = str::from_utf8(data)
                            .map_err(|_| VfsError::InvalidInput)?
                            .trim();
                        let loginuid = if input == "-1" {
                            u32::MAX
                        } else {
                            input.parse::<u32>().map_err(|_| VfsError::InvalidInput)?
                        };
                        task.as_thread().proc_data.set_loginuid(loginuid);
                        Ok(None)
                    }
                }),
            )
            .into(),
            "sessionid" => SimpleFile::new_regular(fs, move || {
                Ok(format!("{}\n", task.as_thread().proc_data.audit_session_id()).into_bytes())
            })
            .into(),
            "task" => SimpleDir::new_maker(
                fs.clone(),
                Arc::new(ProcessTaskDir {
                    fs,
                    process: Arc::downgrade(&task.as_thread().proc_data.proc),
                }),
            )
            .into(),
            "maps" => SimpleFile::new_regular(fs, move || {
                let pid = task.as_thread().proc_data.proc.pid();
                let aspace_ref = task.as_thread().proc_data.aspace();
                let aspace = aspace_ref.lock();
                Ok(crate::syscall::mmap_proc_maps(pid, &aspace))
            })
            .into(),
            "mem" => ProcMemFile::new(fs, Arc::downgrade(&task)).into(),
            "pagemap" => proc_pagemap(fs).into(),
            "mounts" => {
                SimpleFile::new_regular(fs, move || Ok(crate::syscall::proc_mounts())).into()
            }
            "cmdline" => SimpleFile::new_regular(fs, move || {
                let cmdline = task.as_thread().proc_data.cmdline.read();
                let mut buf = Vec::new();
                for arg in cmdline.iter() {
                    buf.extend_from_slice(arg.as_bytes());
                    buf.push(0);
                }
                Ok(buf)
            })
            .into(),
            "auxv" => SimpleFile::new_regular(fs, move || {
                let auxv = task.as_thread().proc_data.auxv.read();
                let mut bytes = Vec::with_capacity((auxv.len() + 1) * 2 * size_of::<usize>());
                for entry in auxv.iter() {
                    bytes.extend_from_slice(&(entry.get_type() as usize).to_ne_bytes());
                    bytes.extend_from_slice(&entry.value().to_ne_bytes());
                }
                bytes.extend_from_slice(&0usize.to_ne_bytes());
                bytes.extend_from_slice(&0usize.to_ne_bytes());
                Ok(bytes)
            })
            .into(),
            "comm" => SimpleFile::new_regular(
                fs,
                RwFile::new(move |req| match req {
                    SimpleFileOperation::Read => {
                        let mut bytes = vec![0; 16];
                        let name = task.name();
                        let copy_len = name.len().min(15);
                        bytes[..copy_len].copy_from_slice(&name.as_bytes()[..copy_len]);
                        bytes[copy_len] = b'\n';
                        Ok(Some(bytes))
                    }
                    SimpleFileOperation::Write(data) => {
                        if !data.is_empty() {
                            let mut input = [0; 16];
                            let copy_len = data.len().min(15);
                            input[..copy_len].copy_from_slice(&data[..copy_len]);
                            task.set_name(
                                CStr::from_bytes_until_nul(&input)
                                    .map_err(|_| VfsError::InvalidInput)?
                                    .to_str()
                                    .map_err(|_| VfsError::InvalidInput)?,
                            );
                        }
                        Ok(None)
                    }
                }),
            )
            .into(),
            "exe" => SimpleFile::new(fs, NodeType::Symlink, move || {
                Ok(task.as_thread().proc_data.exe_path.read().clone())
            })
            .into(),
            "fd" => SimpleDir::new_maker(
                fs.clone(),
                Arc::new(ThreadFdDir {
                    fs,
                    task: Arc::downgrade(&task),
                }),
            )
            .into(),
            "fdinfo" => SimpleDir::new_maker(
                fs.clone(),
                Arc::new(ThreadFdInfoDir {
                    fs,
                    task: Arc::downgrade(&task),
                }),
            )
            .into(),
            "ns" => SimpleDir::new_maker(
                fs.clone(),
                Arc::new(ThreadNamespaceDir {
                    fs,
                    task: Arc::downgrade(&task),
                }),
            )
            .into(),
            "setgroups" => proc_setgroups_file(fs).into(),
            "uid_map" => proc_rw_file(
                fs,
                format!(
                    "         0          {}          1\n",
                    task.as_thread().proc_data.uid()
                ),
            )
            .into(),
            "gid_map" => proc_rw_file(
                fs,
                format!(
                    "         0          {}          1\n",
                    task.as_thread().proc_data.gid()
                ),
            )
            .into(),
            "timens_offsets" => proc_timens_offsets_file(fs).into(),
            _ => return Err(VfsError::NotFound),
        })
    }

    fn is_cacheable(&self) -> bool {
        false
    }
}

struct ProcessOnlyDir {
    fs: Arc<SimpleFs>,
    process: Weak<Process>,
}

impl SimpleDirOps for ProcessOnlyDir {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        Box::new(["stat", "status", "task"].into_iter().map(Cow::Borrowed))
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let fs = self.fs.clone();
        let process = self.process.upgrade().ok_or(VfsError::NotFound)?;
        Ok(match name {
            "stat" => SimpleFile::new_regular(fs, move || {
                Ok(format!("{}", TaskStat::from_process(&process)).into_bytes())
            })
            .into(),
            "status" => SimpleFile::new_regular(fs, move || Ok(process_status(&process))).into(),
            "task" => SimpleDir::new_maker(
                fs.clone(),
                Arc::new(ProcessTaskDir {
                    fs,
                    process: Arc::downgrade(&process),
                }),
            )
            .into(),
            _ => return Err(VfsError::NotFound),
        })
    }

    fn is_cacheable(&self) -> bool {
        false
    }
}

/// Handles /proc/[pid] & /proc/self
struct ProcFsHandler(Arc<SimpleFs>);

impl SimpleDirOps for ProcFsHandler {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        let init_pid = init_proc().pid().to_string();
        let mut names: Vec<Cow<'a, str>> = tasks()
            .into_iter()
            .map(|task| Cow::Owned(task.id().as_u64().to_string()))
            .collect();
        if !names.iter().any(|name| name.as_ref() == init_pid.as_str()) {
            names.push(Cow::Owned(init_pid));
        }
        if !names.iter().any(|name| name.as_ref() == "1") {
            names.push(Cow::Borrowed("1"));
        }
        names.push(Cow::Borrowed("self"));
        Box::new(names.into_iter())
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let task = if name == "self" {
            current().clone()
        } else {
            let tid = name.parse::<u32>().map_err(|_| VfsError::NotFound)?;
            match get_task(tid) {
                Ok(task) => task,
                Err(_) if tid == 1 || tid == init_proc().pid() => {
                    let process = init_proc();
                    return Ok(NodeOpsMux::Dir(SimpleDir::new_maker(
                        self.0.clone(),
                        Arc::new(ProcessOnlyDir {
                            fs: self.0.clone(),
                            process: Arc::downgrade(&process),
                        }),
                    )));
                }
                Err(_) => return Err(VfsError::NotFound),
            }
        };
        let node = NodeOpsMux::Dir(SimpleDir::new_maker(
            self.0.clone(),
            Arc::new(ThreadDir {
                fs: self.0.clone(),
                task: Arc::downgrade(&task),
            }),
        ));
        Ok(node)
    }

    fn is_cacheable(&self) -> bool {
        false
    }
}

fn builder(fs: Arc<SimpleFs>) -> DirMaker {
    let mut root = DirMapping::new();
    root.add(
        "mounts",
        SimpleFile::new_regular(fs.clone(), || Ok(crate::syscall::proc_mounts())),
    );
    root.add(
        "meminfo",
        SimpleFile::new_regular(fs.clone(), || Ok(proc_meminfo())),
    );
    root.add(
        "cpuinfo",
        SimpleFile::new_regular(fs.clone(), || Ok(DUMMY_CPUINFO)),
    );
    root.add(
        "stat",
        SimpleFile::new_regular(fs.clone(), || Ok(proc_stat())),
    );
    root.add(
        "uptime",
        SimpleFile::new_regular(fs.clone(), || Ok(proc_uptime())),
    );
    #[cfg(feature = "net")]
    root.add("net", {
        let mut net = DirMapping::new();
        net.add(
            "dev",
            SimpleFile::new_regular(fs.clone(), || Ok(proc_net_dev())),
        );
        net.add(
            "route",
            SimpleFile::new_regular(fs.clone(), || Ok(proc_net_route())),
        );
        SimpleDir::new_maker(fs.clone(), Arc::new(net))
    });
    root.add(
        "meminfo2",
        SimpleFile::new_regular(fs.clone(), || {
            let allocator = axalloc::global_allocator();
            Ok(format!("{:?}\n", allocator.usages()))
        }),
    );
    root.add(
        "sched_debug",
        SimpleFile::new_regular(fs.clone(), || Ok(DUMMY_SCHED_DEBUG)),
    );
    root.add(
        "key-users",
        SimpleFile::new_regular(fs.clone(), || Ok(crate::syscall::key_users_proc())),
    );
    root.add(
        "instret",
        SimpleFile::new_regular(fs.clone(), || {
            #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
            {
                Ok(format!("{}\n", riscv::register::instret::read64()))
            }
            #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
            {
                Ok("0\n".to_string())
            }
        }),
    );
    {
        static IRQ_CNT: AtomicUsize = AtomicUsize::new(0);

        axtask::register_timer_callback(|_| {
            IRQ_CNT.fetch_add(1, Ordering::Relaxed);
        });

        root.add(
            "interrupts",
            SimpleFile::new_regular(fs.clone(), || {
                Ok(format!("0: {}", IRQ_CNT.load(Ordering::Relaxed)))
            }),
        );
    }

    root.add("sys", {
        let mut sys = DirMapping::new();

        sys.add("kernel", {
            let mut kernel = DirMapping::new();
            let pid_max = Arc::new(Mutex::new(b"4194304\n".to_vec()));

            kernel.add(
                "pid_max",
                SimpleFile::new_regular(
                    fs.clone(),
                    RwFile::new({
                        let pid_max = pid_max.clone();
                        move |op| match op {
                            SimpleFileOperation::Read => Ok(Some(pid_max.lock().clone())),
                            SimpleFileOperation::Write(data) => {
                                let mut value = pid_max.lock();
                                value.clear();
                                value.extend_from_slice(data);
                                Ok(None::<Vec<u8>>)
                            }
                        }
                    }),
                ),
            );
            kernel.add(
                "tainted",
                SimpleFile::new_regular(fs.clone(), StaticFile::new(b"0\n")),
            );
            kernel.add(
                "core_pattern",
                SimpleFile::new_regular(fs.clone(), || Ok("core\n")),
            );
            kernel.add("random", {
                let mut random = DirMapping::new();
                random.add(
                    "entropy_avail",
                    SimpleFile::new_regular(fs.clone(), || Ok("256\n")),
                );
                SimpleDir::new_maker(fs.clone(), Arc::new(random))
            });
            kernel.add("keys", {
                let mut keys = DirMapping::new();

                keys.add("gc_delay", proc_rw_file(fs.clone(), b"1\n".to_vec()));
                keys.add("maxkeys", proc_rw_file(fs.clone(), b"200\n".to_vec()));
                keys.add("maxbytes", proc_rw_file(fs.clone(), b"20000\n".to_vec()));

                SimpleDir::new_maker(fs.clone(), Arc::new(keys))
            });

            SimpleDir::new_maker(fs.clone(), Arc::new(kernel))
        });

        sys.add("fs", {
            let mut fs_sys = DirMapping::new();

            fs_sys.add("pipe-max-size", proc_pipe_max_size_file(fs.clone()));
            fs_sys.add(
                "aio-max-nr",
                SimpleFile::new_regular(fs.clone(), StaticFile::new(b"1024\n")),
            );
            fs_sys.add(
                "aio-nr",
                SimpleFile::new_regular(fs.clone(), StaticFile::new(b"0\n")),
            );
            fs_sys.add(
                "protected_hardlinks",
                proc_bool_sysctl_file(
                    fs.clone(),
                    crate::syscall::protected_hardlinks_enabled,
                    crate::syscall::set_protected_hardlinks,
                ),
            );
            fs_sys.add(
                "protected_symlinks",
                proc_bool_sysctl_file(
                    fs.clone(),
                    crate::syscall::protected_symlinks_enabled,
                    crate::syscall::set_protected_symlinks,
                ),
            );
            fs_sys.add("fanotify", {
                let mut fanotify = DirMapping::new();

                fanotify.add(
                    "max_queued_events",
                    SimpleFile::new_regular(fs.clone(), || {
                        Ok(format!("{}\n", crate::syscall::FANOTIFY_MAX_QUEUED_EVENTS))
                    }),
                );
                fanotify.add(
                    "max_user_groups",
                    SimpleFile::new_regular(fs.clone(), || {
                        Ok(format!("{}\n", crate::syscall::FANOTIFY_MAX_USER_GROUPS))
                    }),
                );
                fanotify.add(
                    "max_user_marks",
                    SimpleFile::new_regular(fs.clone(), || {
                        Ok(format!("{}\n", crate::syscall::FANOTIFY_MAX_USER_MARKS))
                    }),
                );

                SimpleDir::new_maker(fs.clone(), Arc::new(fanotify))
            });
            fs_sys.add("inotify", {
                let mut inotify = DirMapping::new();

                inotify.add(
                    "max_queued_events",
                    proc_usize_sysctl_file(
                        fs.clone(),
                        crate::syscall::inotify_max_queued_events,
                        crate::syscall::set_inotify_max_queued_events,
                    ),
                );
                inotify.add(
                    "max_user_instances",
                    proc_rw_file(fs.clone(), b"8192\n".to_vec()),
                );
                inotify.add(
                    "max_user_watches",
                    proc_rw_file(fs.clone(), b"1048576\n".to_vec()),
                );

                SimpleDir::new_maker(fs.clone(), Arc::new(inotify))
            });

            SimpleDir::new_maker(fs.clone(), Arc::new(fs_sys))
        });

        sys.add("vm", {
            let mut vm = DirMapping::new();

            vm.add(
                "vfs_cache_pressure",
                proc_rw_file(fs.clone(), b"100\n".to_vec()),
            );
            vm.add("drop_caches", {
                let data = Arc::new(Mutex::new(b"0\n".to_vec()));
                SimpleFile::new_regular(
                    fs.clone(),
                    RwFile::new(move |req| match req {
                        SimpleFileOperation::Read => Ok(Some(data.lock().clone())),
                        SimpleFileOperation::Write(input) => {
                            let mut data = data.lock();
                            data.clear();
                            data.extend_from_slice(input);
                            drop_global_file_caches()?;
                            crate::syscall::evict_fanotify_marks();
                            Ok(None::<Vec<u8>>)
                        }
                    }),
                )
            });

            SimpleDir::new_maker(fs.clone(), Arc::new(vm))
        });

        sys.add("user", {
            let mut user = DirMapping::new();

            user.add(
                "max_user_namespaces",
                proc_rw_file(fs.clone(), b"1024\n".to_vec()),
            );
            user.add(
                "max_fanotify_groups",
                proc_usize_sysctl_file(
                    fs.clone(),
                    crate::syscall::fanotify_max_user_groups,
                    crate::syscall::set_fanotify_max_user_groups,
                ),
            );
            user.add(
                "max_fanotify_marks",
                proc_usize_sysctl_file(
                    fs.clone(),
                    crate::syscall::fanotify_max_user_marks,
                    crate::syscall::set_fanotify_max_user_marks,
                ),
            );

            SimpleDir::new_maker(fs.clone(), Arc::new(user))
        });

        SimpleDir::new_maker(fs.clone(), Arc::new(sys))
    });

    let proc_dir = ProcFsHandler(fs.clone());
    SimpleDir::new_maker(fs, Arc::new(proc_dir.chain(root)))
}
