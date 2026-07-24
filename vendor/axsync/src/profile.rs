//! Optional synchronization and I/O wait profiling hooks.

use core::{
    mem,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A wait or latency class recorded by the guest profiler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ProfileEvent {
    /// Time spent asleep while waiting for a blocking mutex.
    MutexWait    = 1,
    /// Time spent inside an ext4 operation, including lock wait and I/O.
    Ext4         = 2,
    /// Time spent in a serialized page-cache operation.
    PageCache    = 3,
    /// Time spent in a synchronous block read.
    BlockRead    = 4,
    /// Time spent in a synchronous block write.
    BlockWrite   = 5,
    /// Time for which the scheduler removes a task from a CPU while a future
    /// is pending. This includes futex, wait4, pipe, socket, mutex and I/O waits.
    OffCpu       = 6,
    /// Contended acquisition time for the filesystem-wide ext4 mutex.
    Ext4LockWait = 7,
    /// Time spent waiting for another task to finish loading a folio.
    FolioLoadWait = 8,
    /// Contended acquisition time for the read side of a file invalidation gate.
    InvalidateReadWait = 9,
    /// Contended acquisition time for the write side of a file invalidation gate.
    InvalidateWriteWait = 10,
    /// End-to-end time spent in a valid `mprotect` syscall.
    MprotectTotal = 11,
    /// Contended acquisition time for the process address-space write lock.
    MprotectAddrSpaceWait = 12,
    /// Contended acquisition time for the mapping-record lock during seal checks.
    MprotectSealLockWait = 13,
    /// Time spent scanning mapping records for a write-sealed shared mapping.
    MprotectSealScan = 14,
    /// Time spent updating page-table permissions.
    MprotectPageTable = 15,
    /// Contended acquisition time for the mapping-record lock before an update.
    MprotectRecordLockWait = 16,
    /// Time spent rebuilding mapping records after permissions change.
    MprotectRecordUpdate = 17,
}

type BeginHook = fn(ProfileEvent, usize) -> u64;
type EndHook = fn(u64);

static BEGIN_HOOK: AtomicUsize = AtomicUsize::new(0);
static END_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Installs the callbacks used to record profiling intervals.
///
/// This is expected to be called once while the kernel is starting.
pub fn register_profile_hooks(begin: BeginHook, end: EndHook) {
    BEGIN_HOOK.store(begin as usize, Ordering::Release);
    END_HOOK.store(end as usize, Ordering::Release);
}

/// Starts one profiling interval and returns its opaque token.
#[inline]
pub fn profile_wait_begin(event: ProfileEvent, object: usize) -> u64 {
    let hook = BEGIN_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return 0;
    }
    // SAFETY: `register_profile_hooks` stores a function pointer of this exact type.
    let hook: BeginHook = unsafe { mem::transmute(hook) };
    hook(event, object)
}

/// Completes a profiling interval created by [`profile_wait_begin`].
#[inline]
pub fn profile_wait_end(token: u64) {
    if token == 0 {
        return;
    }
    let hook = END_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return;
    }
    // SAFETY: `register_profile_hooks` stores a function pointer of this exact type.
    let hook: EndHook = unsafe { mem::transmute(hook) };
    hook(token);
}

/// An RAII profiling interval for an operation with a lexical lifetime.
pub struct ProfileScope(u64);

impl ProfileScope {
    /// Starts a new interval for `event`.
    #[inline]
    pub fn new(event: ProfileEvent, object: usize) -> Self {
        Self(profile_wait_begin(event, object))
    }
}

impl Drop for ProfileScope {
    #[inline]
    fn drop(&mut self) {
        profile_wait_end(self.0);
    }
}
