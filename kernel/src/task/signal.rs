use core::{future::poll_fn, task::Poll};

use axerrno::{AxError, AxResult};
use axhal::uspace::UserContext;
use axtask::{TaskInner, current, future::block_on};
use starry_process::Pid;
use starry_signal::{SignalInfo, SignalOSAction, SignalSet, Signo};

use super::{
    AsThread, ProcessData, ProcessWaitStatus, Thread, do_exit, get_process_data, get_process_group,
    get_task,
};

fn wake_waiting_parent(proc_data: &ProcessData) {
    if let Some(parent) = proc_data.proc.parent()
        && let Ok(parent_data) = get_process_data(parent.pid())
    {
        parent_data.child_exit_event.wake();
    }
}

fn wait_for_continue(thr: &Thread) {
    let proc_data = &thr.proc_data;
    block_on(poll_fn(|cx| {
        if thr.exec_exit_requested() {
            return Poll::Ready(());
        }

        let stopped = matches!(
            *proc_data.wait_status.lock(),
            Some(ProcessWaitStatus::Stopped(_))
        );
        if !stopped || proc_data.proc.is_zombie() {
            Poll::Ready(())
        } else {
            proc_data.stop_event.register(cx.waker());
            // Job-control waits normally wake only through stop_event.  Also
            // register the task interrupt waker so an exec-exit request can
            // make a stopped sibling reach normal thread cleanup.
            let _ = current().poll_interrupt(cx);
            if thr.exec_exit_requested() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Result of checking and dispatching one pending signal.
pub enum SignalCheckResult {
    /// No currently unblocked signal was pending.
    None,
    /// A userspace handler was installed in the supplied context.
    Handler,
    /// A non-handler action, such as stop or termination, was processed.
    Other,
}

/// Checks and dispatches one pending signal, preserving its action kind.
pub fn check_signals_result(
    thr: &Thread,
    uctx: &mut UserContext,
    restore_blocked: Option<SignalSet>,
) -> SignalCheckResult {
    // Once do_exit has removed the TID, the exec leader may replace the
    // address space immediately.  Never drain another signal or construct a
    // user signal frame after either exit state becomes visible.
    if thr.pending_exit() || thr.exec_exit_requested() {
        return SignalCheckResult::None;
    }

    crate::syscall::ptrace_wait_if_stopped(thr);

    if thr.pending_exit() || thr.exec_exit_requested() {
        return SignalCheckResult::None;
    }

    let pid = thr.proc_data.proc.pid();
    if crate::syscall::ptrace_is_traced(pid) {
        let mask = !thr.signal.blocked();
        if let Some(sig) = thr.signal.dequeue_signal(&mask) {
            let signo = sig.signo();
            if crate::syscall::ptrace_intercept_signal(thr, signo) {
                return SignalCheckResult::Other;
            }
            do_exit(signo as i32, true);
            return SignalCheckResult::Other;
        }
    }

    let Some((sig, os_action)) = thr.signal.check_signals(uctx, restore_blocked) else {
        return SignalCheckResult::None;
    };

    let signo = sig.signo();
    match os_action {
        SignalOSAction::Terminate => {
            do_exit(signo as i32, true);
            SignalCheckResult::Other
        }
        SignalOSAction::CoreDump => {
            do_exit((signo as i32) | 0x80, true);
            SignalCheckResult::Other
        }
        SignalOSAction::Stop => {
            *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Stopped(signo));
            wake_waiting_parent(&thr.proc_data);
            wait_for_continue(thr);
            SignalCheckResult::Other
        }
        SignalOSAction::Continue => {
            *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Continued(signo));
            wake_waiting_parent(&thr.proc_data);
            SignalCheckResult::Other
        }
        SignalOSAction::Handler => SignalCheckResult::Handler,
    }
}

pub fn check_signals(
    thr: &Thread,
    uctx: &mut UserContext,
    restore_blocked: Option<SignalSet>,
) -> bool {
    check_signals_result(thr, uctx, restore_blocked) != SignalCheckResult::None
}

pub fn block_next_signal() {
    current().as_thread().suppress_next_signal_check();
}

pub fn unblock_next_signal() -> bool {
    current().as_thread().take_suppress_next_signal_check()
}

pub fn with_blocked_signals<R>(
    blocked: Option<SignalSet>,
    f: impl FnOnce() -> AxResult<R>,
) -> AxResult<R> {
    let curr = current();
    let sig = &curr.as_thread().signal;

    let old_blocked = blocked.map(|set| sig.set_blocked(set));
    f().inspect(|_| {
        if let Some(old) = old_blocked {
            sig.set_blocked(old);
        }
    })
}

pub(super) fn send_signal_thread_inner(task: &TaskInner, thr: &Thread, sig: SignalInfo) {
    if thr.signal.send_signal(sig) {
        task.interrupt();
    }
}

/// Sends a signal to a thread.
pub fn send_signal_to_thread(tgid: Option<Pid>, tid: Pid, sig: Option<SignalInfo>) -> AxResult<()> {
    let task = get_task(tid)?;
    let thread = task.try_as_thread().ok_or(AxError::OperationNotPermitted)?;
    if tgid.is_some_and(|tgid| thread.proc_data.proc.pid() != tgid) {
        return Err(AxError::NoSuchProcess);
    }

    if let Some(sig) = sig {
        let signo = sig.signo();
        info!("Send signal {signo:?} to thread {tid}");
        let pid = thread.proc_data.proc.pid();
        if signo == Signo::SIGCONT {
            *thread.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Continued(signo));
            thread.proc_data.stop_event.wake();
            wake_waiting_parent(&thread.proc_data);
        } else if signo == Signo::SIGKILL {
            thread.proc_data.stop_event.wake();
        }
        if thread.signal.process().signal_ignored(signo)
            && crate::syscall::ptrace_mark_signal_stop(pid, signo)
        {
            return Ok(());
        }
        send_signal_thread_inner(&task, thread, sig);
    }

    Ok(())
}

/// Sends a signal to a process.
pub fn send_signal_to_process(pid: Pid, sig: Option<SignalInfo>) -> AxResult<()> {
    let proc_data = get_process_data(pid)?;

    if let Some(sig) = sig {
        let signo = sig.signo();
        info!("Send signal {signo:?} to process {pid}");
        if signo == Signo::SIGCONT {
            *proc_data.wait_status.lock() = Some(ProcessWaitStatus::Continued(signo));
            proc_data.stop_event.wake();
            wake_waiting_parent(&proc_data);
        } else if signo == Signo::SIGKILL {
            proc_data.stop_event.wake();
        }
        if proc_data.signal.signal_ignored(signo)
            && crate::syscall::ptrace_mark_signal_stop(pid, signo)
        {
            return Ok(());
        }
        if let Some(tid) = proc_data.signal.send_signal(sig)
            && let Ok(task) = get_task(tid)
        {
            task.interrupt();
        }
    }

    Ok(())
}

/// Sends a signal to a process group.
pub fn send_signal_to_process_group(pgid: Pid, sig: Option<SignalInfo>) -> AxResult<()> {
    let pg = get_process_group(pgid)?;

    if let Some(sig) = sig {
        info!("Send signal {:?} to process group {}", sig.signo(), pgid);
        for proc in pg.processes() {
            send_signal_to_process(proc.pid(), Some(sig.clone()))?;
        }
    }

    Ok(())
}

/// Sends a fatal signal to the current process.
pub fn raise_signal_fatal(sig: SignalInfo) -> AxResult<()> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;

    let signo = sig.signo();
    info!("Send fatal signal {signo:?} to the current process");
    if let Some(tid) = proc_data.signal.send_signal(sig)
        && let Ok(task) = get_task(tid)
    {
        task.interrupt();
    } else {
        // No task wants to handle the signal, abort the task
        do_exit(signo as i32, true);
    }

    Ok(())
}
