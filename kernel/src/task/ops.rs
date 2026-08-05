use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{ffi::c_long, sync::atomic::Ordering};

use axerrno::{AxError, AxResult};
use axtask::{AxTaskRef, WeakAxTaskRef, current};
use bytemuck::AnyBitPattern;
use linux_raw_sys::general::ROBUST_LIST_LIMIT;
use spin::RwLock;
use starry_process::{Pid, ProcessGroup, Session};
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr};
use weak_map::WeakMap;

use super::{
    AsThread, FutexKey, ProcessData, TimerState, futex_table_for, send_signal_thread_inner,
    send_signal_to_process, send_signal_to_thread,
};

static TASK_TABLE: RwLock<WeakMap<Pid, WeakAxTaskRef>> = RwLock::new(WeakMap::new());

static PROCESS_TABLE: RwLock<WeakMap<Pid, Weak<ProcessData>>> = RwLock::new(WeakMap::new());

static PROCESS_GROUP_TABLE: RwLock<WeakMap<Pid, Weak<ProcessGroup>>> = RwLock::new(WeakMap::new());

static SESSION_TABLE: RwLock<WeakMap<Pid, Weak<Session>>> = RwLock::new(WeakMap::new());

/// Cleanup expired entries in the task tables.
///
/// This function is intended to be used during memory leak analysis to remove
/// possible noise caused by expired entries in the [`WeakMap`].
pub fn cleanup_task_tables() {
    TASK_TABLE.write().cleanup();
    PROCESS_TABLE.write().cleanup();
    PROCESS_GROUP_TABLE.write().cleanup();
    SESSION_TABLE.write().cleanup();
}

/// Add a process group to the global table.
pub fn add_process_group_to_table(pg: &Arc<ProcessGroup>) {
    PROCESS_GROUP_TABLE.write().insert(pg.pgid(), pg);
}

/// Add a session to the global table.
pub fn add_session_to_table(session: &Arc<Session>) {
    SESSION_TABLE.write().insert(session.sid(), session);
}

/// Add the task, the thread and possibly its process, process group and session
/// to the corresponding tables.
pub fn add_task_to_table(task: &AxTaskRef) {
    let tid = task.id().as_u64() as Pid;

    let mut task_table = TASK_TABLE.write();
    task_table.insert(tid, task);

    let proc_data = &task.as_thread().proc_data;
    let proc = &proc_data.proc;
    let pid = proc.pid();
    let mut proc_table = PROCESS_TABLE.write();
    if proc_table.contains_key(&pid) {
        return;
    }
    proc_table.insert(pid, proc_data);

    let pg = proc.group();
    let mut pg_table = PROCESS_GROUP_TABLE.write();
    if pg_table.contains_key(&pg.pgid()) {
        return;
    }
    pg_table.insert(pg.pgid(), &pg);

    let session = pg.session();
    let mut session_table = SESSION_TABLE.write();
    if session_table.contains_key(&session.sid()) {
        return;
    }
    session_table.insert(session.sid(), &session);
}

/// Lists all tasks.
pub fn tasks() -> Vec<AxTaskRef> {
    TASK_TABLE.read().values().collect()
}

/// Finds the task with the given TID.
pub fn get_task(tid: Pid) -> AxResult<AxTaskRef> {
    if tid == 0 {
        return Ok(current().clone());
    }
    TASK_TABLE.read().get(&tid).ok_or(AxError::NoSuchProcess)
}

/// Lists all processes.
pub fn processes() -> Vec<Arc<ProcessData>> {
    PROCESS_TABLE.read().values().collect()
}

pub(crate) fn address_space_shared_with_live_process(
    aspace: &Arc<crate::mm::AddrSpaceLock>,
    owner_pid: Pid,
) -> bool {
    processes().iter().any(|other| {
        other.proc.pid() != owner_pid
            && !other.proc.is_zombie()
            && Arc::ptr_eq(&other.aspace(), aspace)
    })
}

fn reclaim_process_address_space(proc_data: &ProcessData) {
    let aspace = proc_data.aspace();
    if !address_space_shared_with_live_process(&aspace, proc_data.proc.pid()) {
        crate::mm::defer_address_space_reclaim(aspace);
    }
}

/// Finds the process with the given PID.
pub fn get_process_data(pid: Pid) -> AxResult<Arc<ProcessData>> {
    let curr = current();
    if let Some(thread) = curr.try_as_thread() {
        let proc_data = &thread.proc_data;
        if pid == 0 || pid == proc_data.proc.pid() {
            return Ok(proc_data.clone());
        }
    }
    PROCESS_TABLE.read().get(&pid).ok_or(AxError::NoSuchProcess)
}

/// Finds the process group with the given PGID.
pub fn get_process_group(pgid: Pid) -> AxResult<Arc<ProcessGroup>> {
    PROCESS_GROUP_TABLE
        .read()
        .get(&pgid)
        .ok_or(AxError::NoSuchProcess)
}

/// Finds the session with the given SID.
pub fn get_session(sid: Pid) -> AxResult<Arc<Session>> {
    SESSION_TABLE.read().get(&sid).ok_or(AxError::NoSuchProcess)
}

/// Poll the timer
pub fn poll_timer(task: &AxTaskRef) {
    let Some(thr) = task.try_as_thread() else {
        return;
    };
    let Ok(mut time) = thr.time.try_borrow_mut() else {
        // reentrant borrow, likely IRQ
        return;
    };
    time.poll(task, |signo| {
        send_signal_thread_inner(task, thr, SignalInfo::new_kernel(signo));
    });
}

/// Sets the timer state.
pub fn set_timer_state(task: &AxTaskRef, state: TimerState) {
    let Some(thr) = task.try_as_thread() else {
        return;
    };
    let Ok(mut time) = thr.time.try_borrow_mut() else {
        // reentrant borrow, likely IRQ
        return;
    };
    time.poll(task, |signo| {
        send_signal_thread_inner(task, thr, SignalInfo::new_kernel(signo));
    });
    time.set_state(state);
}

#[repr(C)]
#[derive(Debug, Copy, Clone, AnyBitPattern)]
pub struct RobustList {
    pub next: *mut RobustList,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, AnyBitPattern)]
pub struct RobustListHead {
    pub list: RobustList,
    pub futex_offset: c_long,
    pub list_op_pending: *mut RobustList,
}

fn handle_futex_death(entry: *mut RobustList, offset: i64) -> AxResult<()> {
    let address = (entry as u64)
        .checked_add_signed(offset)
        .ok_or(AxError::InvalidInput)?;
    let address: usize = address.try_into().map_err(|_| AxError::InvalidInput)?;
    let key = FutexKey::new_current(address);

    let futex_table = futex_table_for(&key);

    let Some(futex) = futex_table.get(&key) else {
        return Ok(());
    };
    futex.owner_dead.store(true, Ordering::SeqCst);
    futex.wq.wake(1, u32::MAX);
    Ok(())
}

pub fn exit_robust_list(head: *const RobustListHead) -> AxResult<()> {
    // Reference: https://elixir.bootlin.com/linux/v6.13.6/source/kernel/futex/core.c#L777

    let mut limit = ROBUST_LIST_LIMIT;

    let end_ptr = unsafe { &raw const (*head).list };
    let head = head.vm_read()?;
    let mut entry = head.list.next;
    let offset = head.futex_offset;
    let pending = head.list_op_pending;

    while !core::ptr::eq(entry, end_ptr) {
        let next_entry = entry.vm_read()?.next;
        if entry != pending {
            handle_futex_death(entry, offset)?;
        }
        entry = next_entry;

        limit -= 1;
        if limit == 0 {
            return Err(AxError::FilesystemLoop);
        }
        axtask::yield_now();
    }

    Ok(())
}

pub fn do_exit(exit_code: i32, group_exit: bool) {
    let curr = current();
    let thr = curr.as_thread();

    info!("{} exit with code: {}", curr.id_name(), exit_code);

    let head = thr.robust_list_head() as *const RobustListHead;
    if !head.is_null()
        && let Err(err) = exit_robust_list(head)
    {
        if err.canonicalize() != AxError::BadAddress {
            warn!("exit robust list failed: {err:?}");
        }
    }

    let clear_child_tid = thr.clear_child_tid() as *mut u32;
    if clear_child_tid.vm_write(0).is_ok() {
        let key = FutexKey::new_current(clear_child_tid as usize);
        let table = futex_table_for(&key);
        let guard = table.get(&key);
        if let Some(futex) = guard {
            futex.wq.wake(1, u32::MAX);
        }
        axtask::yield_now();
    }

    let tid = curr.id().as_u64() as Pid;
    thr.signal.detach_from_process();
    let (utime, stime) = {
        let mut time = thr.time.borrow_mut();
        let output = time.output();
        time.cancel_all();
        output
    };

    let process = &thr.proc_data.proc;
    let last_thread = process.exit_thread(tid, exit_code);
    if last_thread {
        process.record_maxrss_kb(
            thr.proc_data
                .current_rss_kb()
                .max(thr.proc_data.child_maxrss_kb()),
        );
        crate::syscall::release_key_quota(thr.proc_data.uid());
        crate::syscall::release_process_aio_resources(process.pid());
        crate::syscall::release_process_cap_state(process.pid());
        crate::syscall::release_process_time_resources(process.pid());
        let aspace = thr.proc_data.aspace();
        let mmap_locked_bytes = if address_space_shared_with_live_process(&aspace, process.pid()) {
            0
        } else {
            aspace.read().locked_bytes()
        };
        crate::syscall::release_process_mmap_resources(process.pid(), mmap_locked_bytes);
        crate::syscall::release_process_fanotify_resources(process.pid());
        crate::syscall::close_all_files_for_current_process();
        reclaim_process_address_space(&thr.proc_data);
        process.exit();
        crate::syscall::ptrace_on_process_exit(process.pid());
        crate::syscall::release_posix_locks_for_pid(process.pid());
        crate::syscall::release_file_leases_for_pid(process.pid());
        if let Some(parent) = process.parent() {
            if let Some(signo) = thr.proc_data.exit_signal {
                let _ = send_signal_to_process(parent.pid(), Some(SignalInfo::new_kernel(signo)));
            }
            if let Ok(data) = get_process_data(parent.pid()) {
                let mut child_time = data.child_cpu_time.lock();
                child_time.0 = child_time
                    .0
                    .saturating_add(to_user_hz_ticks(utime.as_micros()));
                child_time.1 = child_time
                    .1
                    .saturating_add(to_user_hz_ticks(stime.as_micros()));
                data.child_exit_event.wake();
            }
        }
        thr.proc_data.exit_event.wake();

        crate::syscall::SHM_MANAGER
            .lock()
            .clear_proc_shm(process.pid());
    }
    thr.exit_event.wake();

    if group_exit && !process.is_group_exited() {
        process.group_exit();
        let sig = SignalInfo::new_kernel(Signo::SIGKILL);
        for tid in process.threads() {
            let _ = send_signal_to_thread(None, tid, Some(sig.clone()));
        }
    }
    thr.set_exit();
}

fn to_user_hz_ticks(micros: u128) -> u64 {
    const MICROS_PER_TICK: u128 = 10_000;
    if micros == 0 {
        0
    } else {
        ((micros - 1) / MICROS_PER_TICK + 1) as u64
    }
}
