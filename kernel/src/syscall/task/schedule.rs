use axerrno::{AxError, AxResult, LinuxError};
use axhal::time::TimeValue;
use axtask::{
    AxCpuMask, current,
    future::{block_on, interruptible, sleep_until},
};
use linux_raw_sys::general::{
    __kernel_clockid_t, CLOCK_MONOTONIC, CLOCK_PROCESS_CPUTIME_ID, CLOCK_REALTIME,
    CLOCK_THREAD_CPUTIME_ID, PRIO_PGRP, PRIO_PROCESS, PRIO_USER, SCHED_BATCH, SCHED_DEADLINE,
    SCHED_FIFO, SCHED_IDLE, SCHED_NORMAL, SCHED_RESET_ON_FORK, SCHED_RR, TIMER_ABSTIME, timespec,
};
use starry_process::Pid;
use starry_vm::{VmMutPtr, VmPtr, vm_load, vm_write_slice};

use crate::{
    task::{AsThread, ProcessData, get_process_data, get_process_group, get_task},
    time::TimeValueLike,
};

pub fn sys_sched_yield() -> AxResult<isize> {
    axtask::yield_now();
    Ok(0)
}

fn has_unblocked_pending_signal() -> bool {
    let curr = current();
    let signal = &curr.as_thread().signal;
    let pending = signal.pending();
    let blocked = signal.blocked();
    !(pending & !blocked).is_empty()
}

fn sleep_impl(dur: TimeValue) -> (TimeValue, bool) {
    debug!("sleep_impl <= {dur:?}");

    let start = axhal::time::monotonic_time();
    let deadline = axhal::time::wall_time()
        .checked_add(dur)
        .unwrap_or(TimeValue::MAX);

    loop {
        if has_unblocked_pending_signal() {
            let elapsed = axhal::time::monotonic_time().saturating_sub(start);
            return (elapsed, true);
        }

        if axhal::time::wall_time() >= deadline {
            break;
        }

        // A timer wake blocks the task instead of repeatedly yielding through
        // the scheduler. An interrupt without a deliverable signal is stale or
        // spurious, so retry for the remaining interval.
        let _ = block_on(interruptible(sleep_until(deadline)));
    }

    (axhal::time::monotonic_time().saturating_sub(start), false)
}

/// Sleep some nanoseconds
pub fn sys_nanosleep(req: *const timespec, rem: *mut timespec) -> AxResult<isize> {
    // FIXME: AnyBitPattern
    let req = unsafe { req.vm_read_uninit()?.assume_init() }.try_into_time_value()?;
    debug!("sys_nanosleep <= req: {req:?}");

    let (actual, interrupted) = sleep_impl(req);

    if interrupted {
        let diff = req.checked_sub(actual).unwrap_or(TimeValue::from_nanos(0));
        debug!("sys_nanosleep => rem: {diff:?}");
        if let Some(rem) = rem.nullable() {
            rem.vm_write(timespec::from_time_value(diff))?;
        }
        Err(AxError::Interrupted)
    } else {
        Ok(0)
    }
}

pub fn sys_clock_nanosleep(
    clock_id: __kernel_clockid_t,
    flags: u32,
    req: *const timespec,
    rem: *mut timespec,
) -> AxResult<isize> {
    if flags & !TIMER_ABSTIME != 0 {
        return Err(AxError::InvalidInput);
    }

    let clock_id = u32::try_from(clock_id).map_err(|_| AxError::InvalidInput)?;
    let clock = match clock_id {
        CLOCK_REALTIME => axhal::time::realtime_time,
        CLOCK_MONOTONIC => axhal::time::monotonic_time,
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            return Err(AxError::OperationNotSupported);
        }
        _ => {
            warn!("Unsupported clock_id: {clock_id}");
            return Err(AxError::InvalidInput);
        }
    };

    let req = unsafe { req.vm_read_uninit()?.assume_init() }.try_into_time_value()?;
    debug!("sys_clock_nanosleep <= clock_id: {clock_id}, flags: {flags}, req: {req:?}");

    let dur = if flags & TIMER_ABSTIME != 0 {
        req.saturating_sub(clock())
    } else {
        req
    };

    let (actual, interrupted) = sleep_impl(dur);

    if interrupted {
        let diff = dur.checked_sub(actual).unwrap_or(TimeValue::from_nanos(0));
        debug!("sys_clock_nanosleep => rem: {diff:?}");
        if flags & TIMER_ABSTIME == 0
            && let Some(rem) = rem.nullable()
        {
            rem.vm_write(timespec::from_time_value(diff))?;
        }
        Err(AxError::Interrupted)
    } else {
        Ok(0)
    }
}

pub fn sys_sched_getaffinity(pid: i32, cpusetsize: usize, user_mask: *mut u8) -> AxResult<isize> {
    if pid < 0 {
        return Err(AxError::InvalidInput);
    }

    let task = get_task(pid as Pid)?;
    let mask = task.cpumask();
    let mask_bytes = mask.as_bytes();

    if cpusetsize < mask_bytes.len() {
        return Err(AxError::InvalidInput);
    }

    vm_write_slice(user_mask, mask_bytes)?;

    Ok(mask_bytes.len() as _)
}

pub fn sys_sched_setaffinity(pid: i32, cpusetsize: usize, user_mask: *const u8) -> AxResult<isize> {
    if pid < 0 || cpusetsize == 0 {
        return Err(AxError::InvalidInput);
    }

    let size = cpusetsize.min(axconfig::plat::CPU_NUM.div_ceil(8));
    let user_mask = vm_load(user_mask, size)?;
    let mut cpu_mask = AxCpuMask::new();

    for i in 0..(size * 8).min(axconfig::plat::CPU_NUM) {
        if user_mask[i / 8] & (1 << (i % 8)) != 0 {
            cpu_mask.set(i, true);
        }
    }

    if cpu_mask.is_empty() {
        return Err(AxError::InvalidInput);
    }

    let task = get_task(pid as Pid)?;
    let curr = current();
    let curr_proc = &curr.as_thread().proc_data;
    let target_proc = &task.as_thread().proc_data;
    if curr_proc.euid() != 0
        && (curr_proc.euid() != target_proc.euid() || curr_proc.uid() != target_proc.uid())
    {
        return Err(AxError::from(LinuxError::EPERM));
    }

    if task.id() == curr.id() {
        if !axtask::set_current_affinity(cpu_mask) {
            return Err(AxError::InvalidInput);
        }
    } else {
        task.set_cpumask(cpu_mask);
    }

    Ok(0)
}

#[repr(C)]
struct SchedParam {
    sched_priority: i32,
}

fn resolve_sched_process(pid: i32) -> AxResult<alloc::sync::Arc<ProcessData>> {
    if pid < 0 {
        return Err(AxError::InvalidInput);
    }
    get_process_data(pid as Pid)
}

fn base_sched_policy(policy: u32) -> AxResult<u32> {
    match policy {
        SCHED_NORMAL | SCHED_FIFO | SCHED_RR | SCHED_BATCH | SCHED_IDLE => Ok(policy),
        SCHED_DEADLINE => Err(AxError::OperationNotSupported),
        _ => Err(AxError::InvalidInput),
    }
}

fn split_sched_policy(policy: i32) -> AxResult<(u32, u32)> {
    let policy = u32::try_from(policy).map_err(|_| AxError::InvalidInput)?;
    let base = policy & !SCHED_RESET_ON_FORK;
    if policy & !(SCHED_RESET_ON_FORK | base) != 0 {
        return Err(AxError::InvalidInput);
    }
    let base = base_sched_policy(base)?;
    Ok((base, policy & SCHED_RESET_ON_FORK))
}

fn validate_sched_priority(policy: u32, priority: i32) -> AxResult<()> {
    match policy {
        SCHED_FIFO | SCHED_RR if (1..=99).contains(&priority) => Ok(()),
        SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE if priority == 0 => Ok(()),
        _ => Err(AxError::InvalidInput),
    }
}

fn can_set_sched_state(target: &ProcessData, policy: u32, priority: i32) -> bool {
    let curr = current();
    let curr_proc = &curr.as_thread().proc_data;
    if curr_proc.euid() == 0 {
        return true;
    }
    if curr_proc.proc.pid() != target.proc.pid() {
        return false;
    }
    matches!(policy, SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE) && priority == 0
}

pub fn sys_sched_getscheduler(pid: i32) -> AxResult<isize> {
    let proc_data = resolve_sched_process(pid)?;
    Ok(proc_data.sched_policy() as _)
}

pub fn sys_sched_setscheduler(pid: i32, policy: i32, param: *const ()) -> AxResult<isize> {
    let (base_policy, reset_flag) = split_sched_policy(policy)?;
    let proc_data = resolve_sched_process(pid)?;
    let param = unsafe { (param as *const SchedParam).vm_read_uninit()?.assume_init() };
    validate_sched_priority(base_policy, param.sched_priority)?;
    if !can_set_sched_state(&proc_data, base_policy, param.sched_priority) {
        return Err(AxError::from(LinuxError::EPERM));
    }

    proc_data.set_sched_state(base_policy | reset_flag, param.sched_priority);
    Ok(0)
}

pub fn sys_sched_setparam(pid: i32, param: *const ()) -> AxResult<isize> {
    let proc_data = resolve_sched_process(pid)?;
    let param = unsafe { (param as *const SchedParam).vm_read_uninit()?.assume_init() };
    let policy = proc_data.sched_policy() & !SCHED_RESET_ON_FORK;
    validate_sched_priority(policy, param.sched_priority)?;
    if !can_set_sched_state(&proc_data, policy, param.sched_priority) {
        return Err(AxError::from(LinuxError::EPERM));
    }

    proc_data.set_sched_state(proc_data.sched_policy(), param.sched_priority);
    Ok(0)
}

pub fn sys_sched_getparam(pid: i32, param: *mut ()) -> AxResult<isize> {
    let proc_data = resolve_sched_process(pid)?;
    (param as *mut SchedParam).vm_write(SchedParam {
        sched_priority: proc_data.sched_priority(),
    })?;
    Ok(0)
}

pub fn sys_getpriority(which: u32, who: u32) -> AxResult<isize> {
    debug!("sys_getpriority <= which: {which}, who: {who}");

    match which {
        PRIO_PROCESS => {
            if who != 0 {
                let _proc = get_process_data(who)?;
            }
            Ok(20)
        }
        PRIO_PGRP => {
            if who != 0 {
                let _pg = get_process_group(who)?;
            }
            Ok(20)
        }
        PRIO_USER => {
            if who == 0 {
                Ok(20)
            } else {
                Err(AxError::NoSuchProcess)
            }
        }
        _ => Err(AxError::InvalidInput),
    }
}

const IOPRIO_WHO_PROCESS: u32 = 1;
const IOPRIO_CLASS_NONE: u32 = 0;
const IOPRIO_CLASS_RT: u32 = 1;
const IOPRIO_CLASS_BE: u32 = 2;
const IOPRIO_CLASS_IDLE: u32 = 3;
const IOPRIO_CLASS_SHIFT: u32 = 13;
const IOPRIO_PRIO_MASK: u32 = (1 << IOPRIO_CLASS_SHIFT) - 1;
const IOPRIO_PRIO_NUM: u32 = 8;

pub fn sys_ioprio_get(which: u32, who: u32) -> AxResult<isize> {
    if which != IOPRIO_WHO_PROCESS {
        return Err(AxError::InvalidInput);
    }
    Ok(get_process_data(who)?.ioprio() as _)
}

pub fn sys_ioprio_set(which: u32, who: u32, ioprio: u32) -> AxResult<isize> {
    if which != IOPRIO_WHO_PROCESS {
        return Err(AxError::InvalidInput);
    }

    let class = ioprio >> IOPRIO_CLASS_SHIFT;
    let prio = ioprio & IOPRIO_PRIO_MASK;
    match class {
        IOPRIO_CLASS_NONE if prio == 0 => {}
        IOPRIO_CLASS_RT | IOPRIO_CLASS_BE | IOPRIO_CLASS_IDLE if prio < IOPRIO_PRIO_NUM => {}
        _ => return Err(AxError::InvalidInput),
    }

    get_process_data(who)?.set_ioprio(ioprio);
    Ok(0)
}
