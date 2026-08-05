//! User task management.

mod futex;
mod ops;
mod resources;
mod signal;
mod stat;
mod timer;
mod user;

use alloc::{boxed::Box, string::String, sync::Arc, vec, vec::Vec};
use core::{
    cell::RefCell,
    ops::Deref,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use axpoll::PollSet;
use axsync::{Mutex, spin::SpinNoIrq};
use axtask::{TaskExt, TaskInner};
use extern_trait::extern_trait;
use scope_local::{ActiveScope, Scope};
use spin::RwLock;
use starry_process::Process;
use starry_signal::{
    Signo,
    api::{ProcessSignalManager, SignalActions, ThreadSignalManager},
};

pub use self::{futex::*, ops::*, resources::*, signal::*, stat::*, timer::*, user::*};
use crate::mm::AddrSpaceLock;

#[derive(Debug, Clone, Copy)]
pub enum ProcessWaitStatus {
    Stopped(Signo),
    Continued(Signo),
}

const INIT_PID_NS_INODE: u64 = 0x0f00_0001;
const INIT_UTS_NS_INODE: u64 = 0x0f00_0002;
const INIT_USER_NS_INODE: u64 = 0x0f00_0003;
static NEXT_NAMESPACE_INODE: AtomicU64 = AtomicU64::new(0x0f00_0100);

#[derive(Debug, Clone, Copy)]
pub struct NamespaceState {
    pub inode: u64,
    pub parent_inode: Option<u64>,
    pub init_pid: Option<u32>,
}

impl NamespaceState {
    const fn init(inode: u64) -> Self {
        Self {
            inode,
            parent_inode: None,
            init_pid: None,
        }
    }

    fn child_of(parent: Self, init_pid: Option<u32>) -> Self {
        Self {
            inode: NEXT_NAMESPACE_INODE.fetch_add(1, Ordering::Relaxed),
            parent_inode: Some(parent.inode),
            init_pid,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessNamespaces {
    pub pid: NamespaceState,
    pub uts: NamespaceState,
    pub user: NamespaceState,
}

impl ProcessNamespaces {
    const fn init() -> Self {
        Self {
            pid: NamespaceState::init(INIT_PID_NS_INODE),
            uts: NamespaceState::init(INIT_UTS_NS_INODE),
            user: NamespaceState::init(INIT_USER_NS_INODE),
        }
    }
}

///  A wrapper type that assumes the inner type is `Sync`.
#[repr(transparent)]
pub struct AssumeSync<T>(pub T);

unsafe impl<T> Sync for AssumeSync<T> {}

impl<T> Deref for AssumeSync<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The inner data of a thread.
pub struct Thread {
    /// The process data shared by all threads in the process.
    pub proc_data: Arc<ProcessData>,

    /// The clear thread tid field
    ///
    /// See <https://manpages.debian.org/unstable/manpages-dev/set_tid_address.2.en.html#clear_child_tid>
    ///
    /// When the thread exits, the kernel clears the word at this address if it
    /// is not NULL.
    clear_child_tid: AtomicUsize,

    /// The head of the robust list
    robust_list_head: AtomicUsize,

    /// The thread-level signal manager
    pub signal: Arc<ThreadSignalManager>,

    /// Time manager
    ///
    /// This is assumed to be `Sync` because it's only borrowed mutably during
    /// context switches, which is exclusive to the current thread.
    pub time: AssumeSync<RefCell<TimeManager>>,

    /// The OOM score adjustment value.
    oom_score_adj: AtomicI32,

    /// Ready to exit. Cleanup has completed and the user loop may stop.
    pub exit: Arc<AtomicBool>,

    /// This thread must exit because another thread is executing `execve`.
    exec_exit: AtomicBool,

    /// Indicates whether the thread is currently accessing user memory.
    accessing_user_memory: AtomicBool,

    /// Self exit event
    pub exit_event: Arc<PollSet>,
}

impl Thread {
    /// Create a new [`Thread`].
    pub fn new(tid: u32, proc_data: Arc<ProcessData>) -> Box<Self> {
        Box::new(Thread {
            signal: ThreadSignalManager::new(tid, proc_data.signal.clone()),
            proc_data,
            clear_child_tid: AtomicUsize::new(0),
            robust_list_head: AtomicUsize::new(0),
            time: AssumeSync(RefCell::new(TimeManager::new())),
            exit: Arc::new(AtomicBool::new(false)),
            exec_exit: AtomicBool::new(false),
            oom_score_adj: AtomicI32::new(200),
            accessing_user_memory: AtomicBool::new(false),
            exit_event: Arc::default(),
        })
    }

    /// Get the clear child tid field.
    pub fn clear_child_tid(&self) -> usize {
        self.clear_child_tid.load(Ordering::Relaxed)
    }

    /// Set the clear child tid field.
    pub fn set_clear_child_tid(&self, clear_child_tid: usize) {
        self.clear_child_tid
            .store(clear_child_tid, Ordering::Relaxed);
    }

    /// Get the robust list head.
    pub fn robust_list_head(&self) -> usize {
        self.robust_list_head.load(Ordering::SeqCst)
    }

    /// Set the robust list head.
    pub fn set_robust_list_head(&self, robust_list_head: usize) {
        self.robust_list_head
            .store(robust_list_head, Ordering::SeqCst);
    }

    /// Get the oom score adjustment value.
    pub fn oom_score_adj(&self) -> i32 {
        self.oom_score_adj.load(Ordering::SeqCst)
    }

    /// Set the oom score adjustment value.
    pub fn set_oom_score_adj(&self, value: i32) {
        self.oom_score_adj.store(value, Ordering::SeqCst);
    }

    /// Check if the thread is ready to exit.
    pub fn pending_exit(&self) -> bool {
        self.exit.load(Ordering::Acquire)
    }

    /// Ask this thread to exit because another thread is executing `execve`.
    pub fn request_exec_exit(&self) {
        self.exec_exit.store(true, Ordering::Release);
    }

    /// Check whether this thread must exit for `execve`.
    pub fn exec_exit_requested(&self) -> bool {
        self.exec_exit.load(Ordering::Acquire)
    }

    /// Set the thread to exit.
    pub fn set_exit(&self) {
        self.exit.store(true, Ordering::Release);
    }

    /// Check if the thread is accessing user memory.
    pub fn is_accessing_user_memory(&self) -> bool {
        self.accessing_user_memory.load(Ordering::Acquire)
    }

    /// Set the accessing user memory flag.
    pub fn set_accessing_user_memory(&self, accessing: bool) {
        self.accessing_user_memory
            .store(accessing, Ordering::Release);
    }
}

#[extern_trait]
unsafe impl TaskExt for Box<Thread> {
    fn on_enter(&self) {
        let scope = self.proc_data.scope.read();
        unsafe { ActiveScope::set(&scope) };
        core::mem::forget(scope);
    }

    fn on_leave(&self) {
        ActiveScope::set_global();
        unsafe { self.proc_data.scope.force_read_decrement() };
    }
}

/// Helper trait to access the thread from a task.
pub trait AsThread {
    /// Try to get the thread from the task.
    fn try_as_thread(&self) -> Option<&Thread>;

    /// Get the thread from the task, panicking if it is a kernel task.
    fn as_thread(&self) -> &Thread {
        self.try_as_thread().expect("kernel task")
    }
}

impl AsThread for TaskInner {
    fn try_as_thread(&self) -> Option<&Thread> {
        self.task_ext()
            .map(|ext| unsafe { ext.downcast_ref::<Box<Thread>>().as_ref() })
    }
}

/// [`Process`]-shared data.
pub struct ProcessData {
    /// The process.
    pub proc: Arc<Process>,
    /// The executable path
    pub exe_path: RwLock<String>,
    /// The command line arguments
    pub cmdline: RwLock<Arc<Vec<String>>>,
    /// The virtual memory address space.
    // TODO: scopify
    aspace: RwLock<Arc<AddrSpaceLock>>,

    /// Serializes `clone` and `execve` changes to the thread group.
    pub exec_lock: Mutex<()>,
    /// True while this process is replacing its executable image.
    exec_in_progress: AtomicBool,

    /// The resource scope
    pub scope: RwLock<Scope>,
    /// The user heap top
    heap_top: AtomicUsize,

    /// The resource limits
    pub rlim: RwLock<Rlimits>,

    /// The child exit wait event
    pub child_exit_event: Arc<PollSet>,
    /// Self exit event
    pub exit_event: Arc<PollSet>,
    /// Status visible to waitid(WSTOPPED/WCONTINUED).
    pub wait_status: SpinNoIrq<Option<ProcessWaitStatus>>,
    /// Wakes threads blocked by job-control stop signals.
    pub stop_event: Arc<PollSet>,
    /// The exit signal of the thread
    pub exit_signal: Option<Signo>,

    /// The process signal manager
    pub signal: Arc<ProcessSignalManager>,

    /// The futex table.
    futex_table: Arc<FutexTable>,
    /// Accumulated child CPU time in USER_HZ ticks.
    pub child_cpu_time: SpinNoIrq<(u64, u64)>,
    /// Maximum resident set size among waited-for children, in KiB.
    child_maxrss_kb: AtomicUsize,
    namespaces: SpinNoIrq<ProcessNamespaces>,

    personality: AtomicUsize,
    membarrier_state: AtomicU32,
    sched_policy: AtomicU32,
    sched_priority: AtomicI32,

    /// The default mask for file permissions.
    umask: AtomicU32,
    ioprio: AtomicU32,
    uid: AtomicU32,
    euid: AtomicU32,
    suid: AtomicU32,
    gid: AtomicU32,
    egid: AtomicU32,
    sgid: AtomicU32,
    supplementary_groups: SpinNoIrq<Vec<u32>>,
}

impl ProcessData {
    /// Create a new [`ProcessData`].
    pub fn new(
        proc: Arc<Process>,
        exe_path: String,
        cmdline: Arc<Vec<String>>,
        aspace: Arc<AddrSpaceLock>,
        signal_actions: Arc<SpinNoIrq<SignalActions>>,
        exit_signal: Option<Signo>,
    ) -> Arc<Self> {
        Arc::new(Self {
            proc,
            exe_path: RwLock::new(exe_path),
            cmdline: RwLock::new(cmdline),
            aspace: RwLock::new(aspace),
            exec_lock: Mutex::new(()),
            exec_in_progress: AtomicBool::new(false),
            scope: RwLock::new(Scope::new()),
            heap_top: AtomicUsize::new(crate::config::USER_HEAP_BASE),

            rlim: RwLock::default(),

            child_exit_event: Arc::default(),
            exit_event: Arc::default(),
            wait_status: SpinNoIrq::new(None),
            stop_event: Arc::default(),
            exit_signal,

            signal: Arc::new(ProcessSignalManager::new(
                signal_actions,
                crate::config::SIGNAL_TRAMPOLINE,
            )),

            futex_table: Arc::new(FutexTable::new()),
            child_cpu_time: SpinNoIrq::new((0, 0)),
            child_maxrss_kb: AtomicUsize::new(0),
            namespaces: SpinNoIrq::new(ProcessNamespaces::init()),
            personality: AtomicUsize::new(0),
            membarrier_state: AtomicU32::new(0),
            sched_policy: AtomicU32::new(0),
            sched_priority: AtomicI32::new(0),

            umask: AtomicU32::new(0o022),
            ioprio: AtomicU32::new((2 << 13) | 4),
            uid: AtomicU32::new(0),
            euid: AtomicU32::new(0),
            suid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            egid: AtomicU32::new(0),
            sgid: AtomicU32::new(0),
            supplementary_groups: SpinNoIrq::new(vec![0]),
        })
    }

    /// Get the top address of the user heap.
    pub fn get_heap_top(&self) -> usize {
        self.heap_top.load(Ordering::Acquire)
    }

    /// Return a stable reference to the process address space.
    pub fn aspace(&self) -> Arc<AddrSpaceLock> {
        self.aspace.read().clone()
    }

    /// Atomically install a fully constructed replacement address space.
    pub fn replace_aspace(&self, new_aspace: Arc<AddrSpaceLock>) -> Arc<AddrSpaceLock> {
        core::mem::replace(&mut *self.aspace.write(), new_aspace)
    }

    /// Set the top address of the user heap.
    pub fn set_heap_top(&self, top: usize) {
        self.heap_top.store(top, Ordering::Release)
    }

    /// Linux manual: A "clone" child is one which delivers no signal, or a
    /// signal other than SIGCHLD to its parent upon termination.
    pub fn is_clone_child(&self) -> bool {
        self.exit_signal != Some(Signo::SIGCHLD)
    }

    pub fn current_rss_kb(&self) -> usize {
        self.aspace().read().resident_set_size() / 1024
    }

    pub fn child_maxrss_kb(&self) -> usize {
        self.child_maxrss_kb.load(Ordering::Acquire)
    }

    pub fn record_child_maxrss_kb(&self, maxrss_kb: usize) {
        self.child_maxrss_kb.fetch_max(maxrss_kb, Ordering::AcqRel);
    }

    pub fn namespaces(&self) -> ProcessNamespaces {
        *self.namespaces.lock()
    }

    pub fn inherit_namespaces_from(
        &self,
        parent: &ProcessData,
        new_pid: bool,
        new_user: bool,
        new_uts: bool,
    ) {
        let mut namespaces = parent.namespaces();
        if new_pid {
            namespaces.pid = NamespaceState::child_of(namespaces.pid, Some(self.proc.pid()));
        }
        if new_user {
            namespaces.user = NamespaceState::child_of(namespaces.user, None);
        }
        if new_uts {
            namespaces.uts = NamespaceState::child_of(namespaces.uts, None);
        }
        *self.namespaces.lock() = namespaces;
    }

    pub fn visible_pid(&self) -> u32 {
        let namespaces = self.namespaces.lock();
        if namespaces.pid.init_pid == Some(self.proc.pid()) {
            1
        } else {
            self.proc.pid()
        }
    }

    /// Get the umask.
    pub fn umask(&self) -> u32 {
        self.umask.load(Ordering::SeqCst)
    }

    /// Set the umask.
    pub fn set_umask(&self, umask: u32) {
        self.umask.store(umask, Ordering::SeqCst);
    }

    /// Set the umask and return the old value.
    pub fn replace_umask(&self, umask: u32) -> u32 {
        self.umask.swap(umask, Ordering::SeqCst)
    }

    pub fn ioprio(&self) -> u32 {
        self.ioprio.load(Ordering::SeqCst)
    }

    pub fn set_ioprio(&self, ioprio: u32) {
        self.ioprio.store(ioprio, Ordering::SeqCst);
    }

    pub fn personality(&self) -> usize {
        self.personality.load(Ordering::SeqCst)
    }

    pub fn set_personality(&self, personality: usize) {
        self.personality.store(personality, Ordering::SeqCst);
    }

    pub fn membarrier_state(&self) -> u32 {
        self.membarrier_state.load(Ordering::SeqCst)
    }

    pub fn add_membarrier_state(&self, state: u32) {
        self.membarrier_state.fetch_or(state, Ordering::SeqCst);
    }

    pub fn sched_policy(&self) -> u32 {
        self.sched_policy.load(Ordering::SeqCst)
    }

    pub fn sched_priority(&self) -> i32 {
        self.sched_priority.load(Ordering::SeqCst)
    }

    pub fn set_sched_state(&self, policy: u32, priority: i32) {
        self.sched_policy.store(policy, Ordering::SeqCst);
        self.sched_priority.store(priority, Ordering::SeqCst);
    }

    pub fn uid(&self) -> u32 {
        self.uid.load(Ordering::SeqCst)
    }

    pub fn euid(&self) -> u32 {
        self.euid.load(Ordering::SeqCst)
    }

    pub fn suid(&self) -> u32 {
        self.suid.load(Ordering::SeqCst)
    }

    pub fn gid(&self) -> u32 {
        self.gid.load(Ordering::SeqCst)
    }

    pub fn egid(&self) -> u32 {
        self.egid.load(Ordering::SeqCst)
    }

    pub fn sgid(&self) -> u32 {
        self.sgid.load(Ordering::SeqCst)
    }

    pub fn set_uid(&self, uid: u32) {
        self.uid.store(uid, Ordering::SeqCst);
        self.euid.store(uid, Ordering::SeqCst);
        self.suid.store(uid, Ordering::SeqCst);
    }

    pub fn set_real_uid(&self, uid: u32) {
        self.uid.store(uid, Ordering::SeqCst);
    }

    pub fn set_euid(&self, euid: u32) {
        self.euid.store(euid, Ordering::SeqCst);
    }

    pub fn set_suid(&self, suid: u32) {
        self.suid.store(suid, Ordering::SeqCst);
    }

    pub fn set_gid(&self, gid: u32) {
        self.gid.store(gid, Ordering::SeqCst);
        self.egid.store(gid, Ordering::SeqCst);
        self.sgid.store(gid, Ordering::SeqCst);
    }

    pub fn set_real_gid(&self, gid: u32) {
        self.gid.store(gid, Ordering::SeqCst);
    }

    pub fn set_egid(&self, egid: u32) {
        self.egid.store(egid, Ordering::SeqCst);
    }

    pub fn set_sgid(&self, sgid: u32) {
        self.sgid.store(sgid, Ordering::SeqCst);
    }

    pub fn supplementary_groups(&self) -> Vec<u32> {
        self.supplementary_groups.lock().clone()
    }

    pub fn set_supplementary_groups(&self, groups: Vec<u32>) {
        *self.supplementary_groups.lock() = groups;
    }

    /// Check whether this process is replacing its executable image.
    pub fn exec_in_progress(&self) -> bool {
        self.exec_in_progress.load(Ordering::Acquire)
    }

    /// Update the executable-image replacement state.
    pub fn set_exec_in_progress(&self, value: bool) {
        self.exec_in_progress.store(value, Ordering::Release);
    }
}
