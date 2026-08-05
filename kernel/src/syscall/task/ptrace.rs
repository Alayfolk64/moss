use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicBool, Ordering};

use axerrno::{AxError, AxResult};
use axsync::spin::SpinNoIrq;
use axtask::{current, yield_now};
use starry_process::{Pid, init_proc};
use starry_signal::{SignalInfo, Signo};

use crate::task::{AsThread, Thread, get_process_data, send_signal_to_process};

// Linux ptrace request numbers (common across the architectures we run).
const PTRACE_TRACEME: u32 = 0;
const PTRACE_PEEKTEXT: u32 = 1;
const PTRACE_PEEKDATA: u32 = 2;
const PTRACE_PEEKUSER: u32 = 3;
const PTRACE_POKETEXT: u32 = 4;
const PTRACE_POKEDATA: u32 = 5;
const PTRACE_POKEUSER: u32 = 6;
const PTRACE_CONT: u32 = 7;
const PTRACE_KILL: u32 = 8;
const PTRACE_SINGLESTEP: u32 = 9;
const PTRACE_GETREGS: u32 = 12;
const PTRACE_SETREGS: u32 = 13;
const PTRACE_ATTACH: u32 = 16;
const PTRACE_DETACH: u32 = 17;
const PTRACE_SYSCALL: u32 = 24;
const PTRACE_GETSIGINFO: u32 = 0x4202;
const PTRACE_SETSIGINFO: u32 = 0x4203;
const PTRACE_GETREGSET: u32 = 0x4204;
const PTRACE_SETREGSET: u32 = 0x4205;

#[derive(Debug, Clone, Copy)]
struct PtraceState {
    tracer: Pid,
    stopped: bool,
    stop_sig: Signo,
    stop_reported: bool,
}

static PTRACE_TABLE: SpinNoIrq<BTreeMap<Pid, PtraceState>> = SpinNoIrq::new(BTreeMap::new());
static PTRACE_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
pub struct PtraceWaitEvent {
    pub pid: Pid,
    pub status: i32,
}

fn make_stop_status(sig: Signo) -> i32 {
    ((sig as i32) << 8) | 0x7f
}

fn resolve_tracee_pid(pid: Pid) -> Pid {
    if pid == 1 && get_process_data(1).is_err() {
        init_proc().pid()
    } else {
        pid
    }
}

fn pid_matches(raw_pid: i32, tracee: Pid) -> bool {
    if raw_pid == -1 {
        return true;
    }
    if raw_pid > 0 {
        return tracee == resolve_tracee_pid(raw_pid as Pid);
    }
    false
}

fn wake_tracer_waiter(tracer: Pid) {
    if let Ok(proc_data) = get_process_data(tracer) {
        proc_data.child_exit_event.wake();
    }
}

fn mark_stopped(tracee: Pid, sig: Signo) {
    let tracer = {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&tracee) else {
            return;
        };
        state.stopped = true;
        state.stop_sig = sig;
        state.stop_reported = false;
        state.tracer
    };
    wake_tracer_waiter(tracer);
}

fn parse_forward_signal(raw: usize) -> AxResult<Option<Signo>> {
    if raw == 0 {
        return Ok(None);
    }
    Signo::from_repr(raw as u8)
        .ok_or(AxError::InvalidInput)
        .map(Some)
}

fn check_trace_access(tracee: Pid, tracer: Pid) -> AxResult<()> {
    let mut table = PTRACE_TABLE.lock();
    let state = table.get_mut(&tracee).ok_or(AxError::NoSuchProcess)?;
    if state.tracer != tracer {
        return Err(AxError::OperationNotPermitted);
    }
    Ok(())
}

fn allow_attach(curr_euid: u32, target_uid: u32, target_euid: u32) -> bool {
    curr_euid == 0 || curr_euid == target_uid || curr_euid == target_euid
}

pub fn ptrace_has_wait_target(tracer: Pid, raw_pid: i32) -> bool {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let table = PTRACE_TABLE.lock();
    table
        .iter()
        .any(|(tracee, state)| state.tracer == tracer && pid_matches(raw_pid, *tracee))
}

pub fn ptrace_take_wait_event(tracer: Pid, raw_pid: i32) -> Option<PtraceWaitEvent> {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let mut table = PTRACE_TABLE.lock();
    let found = table.iter().find_map(|(tracee, state)| {
        if state.tracer == tracer
            && pid_matches(raw_pid, *tracee)
            && state.stopped
            && !state.stop_reported
        {
            Some((*tracee, state.stop_sig))
        } else {
            None
        }
    })?;

    let (tracee, sig) = found;
    if let Some(state) = table.get_mut(&tracee) {
        state.stop_reported = true;
    }
    Some(PtraceWaitEvent {
        pid: tracee,
        status: make_stop_status(sig),
    })
}

// 被调试进程自己卡住
pub fn ptrace_wait_if_stopped(thr: &Thread) {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return;
    }

    let pid = thr.proc_data.proc.pid();

    loop {
        if thr.pending_exit() || thr.exec_exit_requested() {
            return;
        }

        // Allow SIGKILL to terminate a stopped tracee immediately.
        if thr.signal.pending().has(Signo::SIGKILL) {
            return;
        }

        let stopped = {
            let table = PTRACE_TABLE.lock();
            table.get(&pid).is_some_and(|s| s.stopped)
        };

        if !stopped {
            return;
        }

        yield_now(); //“先让出 CPU，让别人跑” 不是睡死，而是不断让调度器切换
    }
}

pub fn ptrace_is_traced(pid: Pid) -> bool {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let table = PTRACE_TABLE.lock();
    table.contains_key(&pid)
}

pub fn ptrace_mark_signal_stop(pid: Pid, sig: Signo) -> bool {
    if sig == Signo::SIGKILL {
        return false;
    }

    if !ptrace_is_traced(pid) {
        return false;
    }

    mark_stopped(pid, sig);
    true
}

pub fn ptrace_intercept_signal(thr: &Thread, sig: Signo) -> bool {
    let pid = thr.proc_data.proc.pid();
    if !ptrace_mark_signal_stop(pid, sig) {
        return false;
    }

    ptrace_wait_if_stopped(thr);
    true
}

pub fn ptrace_on_exec(pid: Pid) {
    // TRACEME tracees must stop with SIGTRAP after exec.
    mark_stopped(pid, Signo::SIGTRAP);
}

pub fn ptrace_on_process_exit(pid: Pid) {
    let mut table = PTRACE_TABLE.lock();
    table.remove(&pid);

    let traced_by_dead = table
        .iter()
        .filter_map(|(tracee, state)| (state.tracer == pid).then_some(*tracee))
        .collect::<alloc::vec::Vec<_>>();
    for tracee in traced_by_dead {
        table.remove(&tracee);
    }
    PTRACE_ACTIVE.store(!table.is_empty(), Ordering::Release);
}

pub fn sys_ptrace(request: u32, pid: Pid, _addr: usize, data: usize) -> AxResult<isize> {
    let curr = current();
    let curr_thr = curr.as_thread();
    let curr_pid = curr_thr.proc_data.proc.pid();

    match request {
        // 请求让父进程调试自己
        PTRACE_TRACEME => {
            let mut table = PTRACE_TABLE.lock();
            if table.contains_key(&curr_pid) {
                return Err(AxError::OperationNotPermitted);
            }
            let parent = curr_thr
                .proc_data
                .proc
                .parent()
                .ok_or(AxError::OperationNotPermitted)?;
            PTRACE_ACTIVE.store(true, Ordering::Release);
            table.insert(
                curr_pid,
                PtraceState {
                    tracer: parent.pid(),
                    stopped: false,
                    stop_sig: Signo::SIGTRAP,
                    stop_reported: true,
                },
            );
            Ok(0)
        }
        // 当前进程主动附加到另一个进程上，成为它的调试者
        PTRACE_ATTACH => {
            let pid = resolve_tracee_pid(pid);

            if pid == 0 {
                return Err(AxError::NoSuchProcess);
            }
            if pid == curr_pid {
                return Err(AxError::OperationNotPermitted);
            }

            let target = {
                let mut found = None;
                for _ in 0..4096 {
                    match get_process_data(pid) {
                        Ok(proc_data) => {
                            found = Some(proc_data);
                            break;
                        }
                        Err(AxError::NoSuchProcess) => yield_now(),
                        Err(e) => return Err(e),
                    }
                }
                let Some(target) = found else {
                    return Err(AxError::NoSuchProcess);
                };
                target
            };

            let is_init_target = pid == init_proc().pid();
            let is_direct_child = target
                .proc
                .parent()
                .is_some_and(|parent| parent.pid() == curr_pid);
            if !is_init_target && !is_direct_child {
                return Err(AxError::OperationNotPermitted);
            }

            if !allow_attach(curr_thr.proc_data.euid(), target.uid(), target.euid()) {
                return Err(AxError::OperationNotPermitted);
            }

            let mut table = PTRACE_TABLE.lock();
            if table.contains_key(&pid) {
                return Err(AxError::OperationNotPermitted);
            }

            PTRACE_ACTIVE.store(true, Ordering::Release);
            table.insert(
                pid,
                PtraceState {
                    tracer: curr_pid,
                    stopped: true,
                    stop_sig: Signo::SIGSTOP,
                    stop_reported: false,
                },
            );
            Ok(0)
        }
        // 让已经暂停的被调试进程继续运行
        PTRACE_CONT | PTRACE_SYSCALL | PTRACE_SINGLESTEP => {
            let pid = resolve_tracee_pid(pid);
            get_process_data(pid)?;
            check_trace_access(pid, curr_pid)?;

            let sig = parse_forward_signal(data)?;
            {
                let mut table = PTRACE_TABLE.lock();
                if let Some(state) = table.get_mut(&pid) {
                    state.stopped = false;
                }
            }
            if let Some(sig) = sig {
                send_signal_to_process(pid, Some(SignalInfo::new_kernel(sig)))?;
            }
            Ok(0)
        }
        // 解除调试关系，让目标不再受 ptrace 控制
        PTRACE_DETACH => {
            let pid = resolve_tracee_pid(pid);
            get_process_data(pid)?;
            check_trace_access(pid, curr_pid)?;

            let sig = parse_forward_signal(data)?;
            {
                let mut table = PTRACE_TABLE.lock();
                table.remove(&pid);
                PTRACE_ACTIVE.store(!table.is_empty(), Ordering::Release);
            }
            if let Some(sig) = sig {
                send_signal_to_process(pid, Some(SignalInfo::new_kernel(sig)))?;
            }
            Ok(0)
        }
        // 向被调试进程发送 SIGKILL，强制杀掉它
        PTRACE_KILL => {
            let pid = resolve_tracee_pid(pid);
            get_process_data(pid)?;
            check_trace_access(pid, curr_pid)?;
            {
                let mut table = PTRACE_TABLE.lock();
                if let Some(state) = table.get_mut(&pid) {
                    state.stopped = false;
                }
            }
            send_signal_to_process(pid, Some(SignalInfo::new_kernel(Signo::SIGKILL)))?;
            Ok(0)
        }
        // Minimal behavior to satisfy ptrace06: bad access must fail with EIO/EFAULT.
        // Linux 原意是读目标进程代码、数据、用户寄存器区。MOSS 这里只做权限检查，然后返回 EIO，表示不支持真实读取
        PTRACE_PEEKTEXT | PTRACE_PEEKDATA | PTRACE_PEEKUSER | PTRACE_POKETEXT | PTRACE_POKEDATA
        | PTRACE_POKEUSER | PTRACE_GETREGS | PTRACE_SETREGS | PTRACE_GETSIGINFO
        | PTRACE_SETSIGINFO | PTRACE_GETREGSET | PTRACE_SETREGSET => {
            let pid = resolve_tracee_pid(pid);
            get_process_data(pid)?;
            check_trace_access(pid, curr_pid)?;
            Err(AxError::Io)
        }
        _ => Err(AxError::InvalidInput),
    }
}
