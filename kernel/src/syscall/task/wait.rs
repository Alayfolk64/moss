use alloc::{sync::Arc, vec::Vec};
use core::{future::poll_fn, mem, task::Poll};

use axerrno::{AxError, AxResult, LinuxError};
use axhal::uspace::UserContext;
use axtask::{
    current,
    future::{block_on, interruptible},
    yield_now,
};
use bitflags::bitflags;
use linux_raw_sys::general::{
    __WALL, __WCLONE, __WNOTHREAD, CLD_CONTINUED, CLD_DUMPED, CLD_EXITED, CLD_KILLED, CLD_STOPPED,
    P_ALL, P_PGID, P_PID, P_PIDFD, WCONTINUED, WEXITED, WNOHANG, WNOWAIT, WUNTRACED, siginfo_t,
};
use starry_process::{Pid, Process};
use starry_signal::Signo;
use starry_vm::{VmMutPtr, VmPtr};

use super::{ptrace_has_wait_target, ptrace_take_wait_event};
use crate::{
    file::{FileLike, PidFd},
    syscall::SYSCALL_RESTART,
    task::{AsThread, ProcessData, ProcessWaitStatus, get_process_data},
};

fn restart_syscall(uctx: &mut UserContext) {
    #[cfg(target_arch = "x86_64")]
    const SYSCALL_INSN_SIZE: usize = 2;
    #[cfg(not(target_arch = "x86_64"))]
    const SYSCALL_INSN_SIZE: usize = 4;

    let ip = uctx.ip();
    uctx.set_ip(ip.saturating_sub(SYSCALL_INSN_SIZE));
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    struct WaitOptions: u32 {
        /// Do not block when there are no processes wishing to report status.
        const WNOHANG = WNOHANG;
        /// Report the status of selected processes which are stopped due to a
        /// `SIGTTIN`, `SIGTTOU`, `SIGTSTP`, or `SIGSTOP` signal.
        const WUNTRACED = WUNTRACED;
        /// Report the status of selected processes which have terminated.
        const WEXITED = WEXITED;
        /// Report the status of selected processes that have continued from a
        /// job control stop by receiving a `SIGCONT` signal.
        const WCONTINUED = WCONTINUED;
        /// Don't reap, just poll status.
        const WNOWAIT = WNOWAIT;

        /// Don't wait on children of other threads in this group
        const WNOTHREAD = __WNOTHREAD;
        /// Wait on all children, regardless of type
        const WALL = __WALL;
        /// Wait for "clone" children only.
        const WCLONE = __WCLONE;
    }
}

#[derive(Debug, Clone, Copy)]
enum WaitPid {
    /// Wait for any child process
    Any,
    /// Wait for the child whose process ID is equal to the value.
    Pid(Pid),
    /// Wait for any child process whose process group ID is equal to the value.
    Pgid(Pid),
}

fn validate_waitid_options(raw_options: u32) -> AxResult<WaitOptions> {
    let options = WaitOptions::from_bits(raw_options).ok_or(AxError::InvalidInput)?;
    let waits_for_status =
        options.intersects(WaitOptions::WEXITED | WaitOptions::WUNTRACED | WaitOptions::WCONTINUED);
    if !waits_for_status {
        return Err(AxError::InvalidInput);
    }
    Ok(options)
}

fn validate_waitpid_options(raw_options: u32) -> AxResult<WaitOptions> {
    let allowed = WaitOptions::WNOHANG
        | WaitOptions::WUNTRACED
        | WaitOptions::WCONTINUED
        | WaitOptions::WNOTHREAD
        | WaitOptions::WALL
        | WaitOptions::WCLONE;
    let options = WaitOptions::from_bits(raw_options).ok_or(AxError::InvalidInput)?;
    if options.bits() & !allowed.bits() != 0 {
        return Err(AxError::InvalidInput);
    }
    Ok(options)
}

fn make_sigchld_info(pid: Pid, status: i32, code: u32) -> siginfo_t {
    // SAFETY: siginfo_t is a plain C data structure and the zero value is valid.
    let mut info: siginfo_t = unsafe { mem::zeroed() };
    info.__bindgen_anon_1.__bindgen_anon_1.si_signo = Signo::SIGCHLD as _;
    info.__bindgen_anon_1.__bindgen_anon_1.si_code = code as _;
    info.__bindgen_anon_1
        .__bindgen_anon_1
        ._sifields
        ._sigchld
        ._pid = pid as _;
    info.__bindgen_anon_1
        .__bindgen_anon_1
        ._sifields
        ._sigchld
        ._status = status;
    info
}

fn make_exit_siginfo(child: &Process) -> siginfo_t {
    let wait_status = child.exit_code();
    let term_sig = wait_status & 0x7f;
    let (status, code) = if term_sig == 0 {
        (wait_status >> 8, CLD_EXITED)
    } else if wait_status & 0x80 != 0 {
        (term_sig, CLD_DUMPED)
    } else {
        (term_sig, CLD_KILLED)
    };
    make_sigchld_info(child.pid(), status, code)
}

fn wait_status_siginfo(child: &Process, options: WaitOptions) -> AxResult<Option<siginfo_t>> {
    let Ok(proc_data) = get_process_data(child.pid()) else {
        return Ok(None);
    };
    let mut status = proc_data.wait_status.lock();
    let Some(wait_status) = *status else {
        return Ok(None);
    };

    match wait_status {
        ProcessWaitStatus::Stopped(signo) if options.contains(WaitOptions::WUNTRACED) => {
            if !options.contains(WaitOptions::WNOWAIT) {
                *status = None;
                proc_data.stop_event.wake();
            }
            Ok(Some(make_sigchld_info(
                child.pid(),
                signo as i32,
                CLD_STOPPED,
            )))
        }
        ProcessWaitStatus::Continued(signo) if options.contains(WaitOptions::WCONTINUED) => {
            if !options.contains(WaitOptions::WNOWAIT) {
                *status = None;
            }
            Ok(Some(make_sigchld_info(
                child.pid(),
                signo as i32,
                CLD_CONTINUED,
            )))
        }
        _ => Ok(None),
    }
}

fn stopped_wait_status(signo: Signo) -> i32 {
    ((signo as i32) << 8) | 0x7f
}

fn continued_wait_status() -> i32 {
    0xffff
}

fn wait_status_waitpid(child: &Process, options: WaitOptions) -> AxResult<Option<i32>> {
    let Ok(proc_data) = get_process_data(child.pid()) else {
        return Ok(None);
    };
    let mut status = proc_data.wait_status.lock();
    let Some(wait_status) = *status else {
        return Ok(None);
    };

    match wait_status {
        ProcessWaitStatus::Stopped(signo) if options.contains(WaitOptions::WUNTRACED) => {
            *status = None;
            proc_data.stop_event.wake();
            Ok(Some(stopped_wait_status(signo)))
        }
        ProcessWaitStatus::Continued(_) if options.contains(WaitOptions::WCONTINUED) => {
            *status = None;
            Ok(Some(continued_wait_status()))
        }
        _ => Ok(None),
    }
}

fn reap_exited_child(parent: &ProcessData, child: &Process) {
    if !parent.signal.signal_ignored(Signo::SIGCHLD) {
        parent.record_child_maxrss_kb(child.maxrss_kb());
    }
    child.free();
}

fn waitid_children(
    children: Vec<Arc<Process>>,
    infop: *mut siginfo_t,
    options: WaitOptions,
    pidfd_nonblocking: bool,
) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;

    let check_children = || -> AxResult<Option<isize>> {
        for child in &children {
            if let Some(info) = wait_status_siginfo(child, options)? {
                if let Some(infop) = infop.nullable() {
                    infop.vm_write(info)?;
                }
                return Ok(Some(0));
            }
        }

        if options.contains(WaitOptions::WEXITED)
            && let Some(child) = children.iter().find(|child| child.is_zombie())
        {
            if let Some(infop) = infop.nullable() {
                infop.vm_write(make_exit_siginfo(child))?;
            }
            if !options.contains(WaitOptions::WNOWAIT) {
                reap_exited_child(proc_data, child);
            }
            return Ok(Some(0));
        }

        if options.contains(WaitOptions::WNOHANG) {
            Ok(Some(0))
        } else if pidfd_nonblocking {
            Err(AxError::WouldBlock)
        } else {
            Ok(None)
        }
    };

    loop {
        match block_on(interruptible(poll_fn(|cx| {
            match check_children().transpose() {
                Some(res) => Poll::Ready(res),
                None => {
                    proc_data.child_exit_event.register(cx.waker());
                    match check_children().transpose() {
                        Some(res) => Poll::Ready(res),
                        None => Poll::Pending,
                    }
                }
            }
        }))) {
            Ok(res) => return res,
            Err(_) => {
                if let Some(res) = check_children().transpose() {
                    return res;
                }

                let pending = curr.as_thread().signal.pending();
                if pending.has(Signo::SIGCHLD) {
                    continue;
                }
                if pending.is_empty() {
                    continue;
                }
                return Err(AxError::Interrupted);
            }
        }
    }
}

pub fn sys_waitid(
    idtype: i32,
    id: u32,
    infop: *mut siginfo_t,
    options: u32,
    _rusage: usize,
) -> AxResult<isize> {
    let options = validate_waitid_options(options)?;
    info!("sys_waitid <= idtype: {idtype:?}, id: {id:?}, options: {options:?}");

    let curr = current();
    let proc = &curr.as_thread().proc_data.proc;

    let (children, pidfd_nonblocking) = match idtype as u32 {
        P_ALL => (proc.children(), false),
        P_PID => (
            proc.children()
                .into_iter()
                .filter(|child| child.pid() == id)
                .collect(),
            false,
        ),
        P_PGID => {
            let pgid = if id == 0 { proc.group().pgid() } else { id };
            (
                proc.children()
                    .into_iter()
                    .filter(|child| child.group().pgid() == pgid)
                    .collect(),
                false,
            )
        }
        P_PIDFD => {
            let pidfd = PidFd::from_fd(id as _)?;
            let pidfd_nonblocking = pidfd.nonblocking();
            let child_pid = pidfd.process_data()?.proc.pid();
            let children = proc
                .children()
                .into_iter()
                .filter(|child| child.pid() == child_pid)
                .collect();
            (children, pidfd_nonblocking)
        }
        _ => return Err(AxError::InvalidInput),
    };

    if children.is_empty() {
        return Err(AxError::from(LinuxError::ECHILD));
    }

    waitid_children(children, infop, options, pidfd_nonblocking)
}

impl WaitPid {
    fn apply(&self, child: &Process) -> bool {
        match self {
            WaitPid::Any => true,
            WaitPid::Pid(pid) => child.pid() == *pid,
            WaitPid::Pgid(pgid) => child.group().pgid() == *pgid,
        }
    }
}

pub fn sys_waitpid(
    uctx: &mut UserContext,
    pid: i32,
    exit_code: *mut i32,
    options: u32,
) -> AxResult<isize> {
    let options = validate_waitpid_options(options)?;
    info!("sys_waitpid <= pid: {pid:?}, options: {options:?}");
    let raw_pid = pid;

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let proc = &proc_data.proc;

    let pid = if pid == -1 {
        WaitPid::Any
    } else if pid == 0 {
        WaitPid::Pgid(proc.group().pgid())
    } else if pid > 0 {
        WaitPid::Pid(pid as _)
    } else if pid == i32::MIN {
        return Err(AxError::NoSuchProcess);
    } else {
        WaitPid::Pgid(-pid as _)
    };

    // FIXME: add back support for WALL & WCLONE, since ProcessData may drop before
    // Process now.
    let children = proc
        .children()
        .into_iter()
        .filter(|child| pid.apply(child))
        .collect::<Vec<_>>();
    if children.is_empty() && !ptrace_has_wait_target(proc.pid(), raw_pid) {
        return Err(AxError::from(LinuxError::ECHILD));
    }

    let check_children = || {
        for child in &children {
            if let Some(status) = wait_status_waitpid(child, options)? {
                if let Some(exit_code) = exit_code.nullable() {
                    exit_code.vm_write(status)?;
                }
                return Ok(Some(child.pid() as _));
            }
        }

        if let Some(child) = children.iter().find(|child| child.is_zombie()) {
            if !options.contains(WaitOptions::WNOWAIT) {
                reap_exited_child(proc_data, child);
            }
            if let Some(exit_code) = exit_code.nullable() {
                exit_code.vm_write(child.exit_code())?;
            }
            Ok(Some(child.pid() as _))
        } else if options.contains(WaitOptions::WNOHANG) {
            yield_now();
            Ok(Some(0))
        } else {
            Ok(None)
        }
    };

    let check_ptrace = || -> AxResult<Option<isize>> {
        if let Some(event) = ptrace_take_wait_event(proc.pid(), raw_pid) {
            if let Some(exit_code) = exit_code.nullable() {
                exit_code.vm_write(event.status)?;
            }
            Ok(Some(event.pid as _))
        } else {
            Ok(None)
        }
    };

    loop {
        match block_on(interruptible(poll_fn(|cx| {
            match check_ptrace().transpose() {
                Some(res) => Poll::Ready(res),
                None => match check_children().transpose() {
                    Some(res) => Poll::Ready(res),
                    None => {
                        proc_data.child_exit_event.register(cx.waker());
                        match check_ptrace().transpose() {
                            Some(res) => Poll::Ready(res),
                            None => match check_children().transpose() {
                                Some(res) => Poll::Ready(res),
                                None => Poll::Pending,
                            },
                        }
                    }
                },
            }
        }))) {
            Ok(res) => return res,
            Err(_) => {
                if let Some(res) = check_ptrace().transpose() {
                    return res;
                }

                if let Some(res) = check_children().transpose() {
                    return res;
                }

                let pending = curr.as_thread().signal.pending();
                if pending.has(Signo::SIGCHLD) {
                    debug!("sys_waitpid: interrupted by pending SIGCHLD, retrying");
                    continue;
                }

                // Some signals should restart blocking syscalls (SA_RESTART).
                // Retry waitpid in that case to match Linux behavior.
                let restartable = (1u8..=64)
                    .filter_map(Signo::from_repr)
                    .any(|signo| pending.has(signo) && proc_data.signal.can_restart(signo));
                if restartable {
                    debug!("sys_waitpid: interrupted by restartable signal, restarting");
                    restart_syscall(uctx);
                    return Ok(SYSCALL_RESTART);
                }

                // Spurious wakeup / already-consumed interrupt, keep waiting.
                if pending.is_empty() {
                    continue;
                }

                // Control signals must break out so user-space handlers
                // (timeout/Ctrl-C paths) can run promptly.
                if pending.has(Signo::SIGALRM)
                    || pending.has(Signo::SIGINT)
                    || pending.has(Signo::SIGTERM)
                {
                    return Err(AxError::Interrupted);
                }

                // Restart waitpid for pure SA_RESTART-style interrupts
                // (e.g. LTP heartbeat SIGUSR1 in the harness parent).
                let all_restartable = (1u8..=64)
                    .filter_map(Signo::from_repr)
                    .filter(|&signo| pending.has(signo))
                    .all(|signo| proc_data.signal.can_restart(signo));
                if all_restartable {
                    restart_syscall(uctx);
                    return Ok(SYSCALL_RESTART);
                }

                return Err(AxError::Interrupted);
            }
        }
    }
}
