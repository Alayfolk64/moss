use core::{future::poll_fn, task::Poll};

use axerrno::{AxError, AxResult, LinuxError};
use axhal::uspace::UserContext;
use axtask::{
    current,
    future::{self, block_on},
};
use linux_raw_sys::general::{
    MINSIGSTKSZ, SI_TKILL, SI_USER, SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK, SS_AUTODISARM, SS_DISABLE,
    kernel_sigaction, siginfo, timespec,
};
use starry_process::Pid;
use starry_signal::{SignalInfo, SignalSet, SignalStack, Signo};
use starry_vm::{VmMutPtr, VmPtr};

use crate::{
    task::{
        AsThread, ProcessData, block_next_signal, check_signals, get_process_data,
        get_process_group, processes, send_signal_to_process, send_signal_to_thread,
    },
    time::TimeValueLike,
};

pub(crate) fn check_sigset_size(size: usize) -> AxResult<()> {
    if size != size_of::<SignalSet>() && size != 0 {
        return Err(AxError::InvalidInput);
    }
    Ok(())
}

fn parse_signo(signo: u32) -> AxResult<Signo> {
    Signo::from_repr(signo as u8).ok_or(AxError::InvalidInput)
}

pub fn sys_rt_sigprocmask(
    how: i32,
    set: *const SignalSet,
    oldset: *mut SignalSet,
    sigsetsize: usize,
) -> AxResult<isize> {
    check_sigset_size(sigsetsize)?;

    let curr = current();
    let sig = &curr.as_thread().signal;
    let old = sig.blocked();

    if let Some(oldset) = oldset.nullable() {
        oldset.vm_write(old)?;
    }

    if let Some(set) = set.nullable() {
        let set = unsafe { set.vm_read_uninit()?.assume_init() };

        let set = match how as u32 {
            SIG_BLOCK => old | set,
            SIG_UNBLOCK => old & !set,
            SIG_SETMASK => set,
            _ => return Err(AxError::InvalidInput),
        };

        debug!("sys_rt_sigprocmask <= {set:?}");
        sig.set_blocked(set);
    }

    Ok(0)
}

pub fn sys_rt_sigaction(
    signo: u32,
    act: *const kernel_sigaction,
    oldact: *mut kernel_sigaction,
    sigsetsize: usize,
) -> AxResult<isize> {
    check_sigset_size(sigsetsize)?;

    let signo = parse_signo(signo)?;
    if matches!(signo, Signo::SIGKILL | Signo::SIGSTOP) {
        return Err(AxError::InvalidInput);
    }

    let curr = current();
    let mut actions = curr.as_thread().proc_data.signal.actions.lock();
    if let Some(oldact) = oldact.nullable() {
        oldact.vm_write(actions[signo].clone().into())?;
    }
    if let Some(act) = act.nullable() {
        let act = unsafe { act.vm_read_uninit()?.assume_init() }.into();
        debug!("sys_rt_sigaction <= signo: {signo:?}, act: {act:?}");
        actions[signo] = act;
    }
    Ok(0)
}

pub fn sys_rt_sigpending(set: *mut SignalSet, sigsetsize: usize) -> AxResult<isize> {
    check_sigset_size(sigsetsize)?;
    set.vm_write(current().as_thread().signal.pending())?;
    Ok(0)
}

fn make_siginfo(signo: u32, code: i32) -> AxResult<Option<SignalInfo>> {
    if signo == 0 {
        return Ok(None);
    }
    let signo = parse_signo(signo)?;
    Ok(Some(SignalInfo::new_user(
        signo,
        code,
        current().as_thread().proc_data.proc.pid(),
    )))
}

fn can_send_signal(caller: &ProcessData, target: &ProcessData, signo: Option<Signo>) -> bool {
    caller.euid() == 0
        || caller.uid() == target.uid()
        || caller.uid() == target.suid()
        || caller.euid() == target.uid()
        || caller.euid() == target.suid()
        || (signo == Some(Signo::SIGCONT)
            && caller.proc.group().session().sid() == target.proc.group().session().sid())
}

fn check_send_signal_permission(
    caller: &ProcessData,
    target: &ProcessData,
    signo: Option<Signo>,
) -> AxResult<()> {
    if can_send_signal(caller, target, signo) {
        Ok(())
    } else {
        Err(AxError::from(LinuxError::EPERM))
    }
}

fn send_signal_to_process_checked(
    caller: &ProcessData,
    target: &ProcessData,
    sig: Option<SignalInfo>,
) -> AxResult<()> {
    let signo = sig.as_ref().map(|sig| sig.signo());
    check_send_signal_permission(caller, target, signo)?;
    if let Some(sig) = sig {
        send_signal_to_process(target.proc.pid(), Some(sig))?;
    }
    Ok(())
}

fn send_signal_to_process_group_checked(
    caller: &ProcessData,
    pgid: Pid,
    sig: Option<SignalInfo>,
) -> AxResult<()> {
    let pg = get_process_group(pgid)?;
    let signo = sig.as_ref().map(|sig| sig.signo());
    let mut found = false;
    let mut permitted = false;

    for proc in pg.processes() {
        let Ok(target) = get_process_data(proc.pid()) else {
            continue;
        };
        found = true;

        if can_send_signal(caller, &target, signo) {
            permitted = true;
            if let Some(sig) = sig.as_ref() {
                let _ = send_signal_to_process(target.proc.pid(), Some(sig.clone()));
            }
        }
    }

    if permitted {
        Ok(())
    } else if found {
        Err(AxError::from(LinuxError::EPERM))
    } else {
        Err(AxError::NoSuchProcess)
    }
}

pub fn sys_kill(pid: i32, signo: u32) -> AxResult<isize> {
    debug!("sys_kill: pid = {pid}, signo = {signo}");
    let sig = make_siginfo(signo, SI_USER as _)?;
    let sig_signo = sig.as_ref().map(|sig| sig.signo());
    let curr = current();
    let caller = &curr.as_thread().proc_data;

    match pid {
        1.. => {
            let target = get_process_data(pid as _)?;
            send_signal_to_process_checked(caller, &target, sig)?;
        }
        0 => {
            let pgid = caller.proc.group().pgid();
            send_signal_to_process_group_checked(caller, pgid, sig)?;
        }
        -1 => {
            let curr_pid = caller.proc.pid();
            let mut found = false;
            let mut permitted = false;

            for target in processes() {
                // POSIX.1 requires that kill(-1,sig) send sig to all processes that
                // the calling process may send signals to, except possibly for some
                // implementation-defined system processes.  Linux does not signal
                // the caller itself in this case.
                if target.proc.is_init() || target.proc.pid() == curr_pid {
                    continue;
                }
                found = true;

                if can_send_signal(caller, &target, sig_signo) {
                    permitted = true;
                    if let Some(sig) = sig.as_ref() {
                        let _ = send_signal_to_process(target.proc.pid(), Some(sig.clone()));
                    }
                }
            }

            if !permitted {
                return if found {
                    Err(AxError::from(LinuxError::EPERM))
                } else {
                    Err(AxError::NoSuchProcess)
                };
            }
        }
        ..-1 => {
            let pgid = pid.checked_neg().ok_or(AxError::NoSuchProcess)? as Pid;
            send_signal_to_process_group_checked(caller, pgid, sig)?;
        }
    }
    Ok(0)
}

pub fn sys_tkill(tid: i32, signo: u32) -> AxResult<isize> {
    if tid <= 0 {
        return Err(AxError::InvalidInput);
    }
    let sig = make_siginfo(signo, SI_TKILL)?;
    send_signal_to_thread(None, tid as Pid, sig)?;
    Ok(0)
}

pub fn sys_tgkill(tgid: i32, tid: i32, signo: u32) -> AxResult<isize> {
    if tgid <= 0 || tid <= 0 {
        return Err(AxError::InvalidInput);
    }
    let sig = make_siginfo(signo, SI_TKILL)?;
    send_signal_to_thread(Some(tgid as Pid), tid as Pid, sig)?;
    Ok(0)
}

pub(crate) fn make_queue_signal_info(
    tgid: Pid,
    signo: u32,
    sig: *const SignalInfo,
) -> AxResult<Option<SignalInfo>> {
    if signo == 0 {
        return Ok(None);
    }

    let signo = parse_signo(signo)?;
    let mut sig = unsafe { sig.vm_read_uninit()?.assume_init() };
    sig.set_signo(signo);
    if current().as_thread().proc_data.proc.pid() != tgid
        && (sig.code() >= 0 || sig.code() == SI_TKILL)
    {
        return Err(AxError::OperationNotPermitted);
    }
    Ok(Some(sig))
}

pub fn sys_rt_sigqueueinfo(
    tgid: Pid,
    signo: u32,
    sig: *const SignalInfo,
    sigsetsize: usize,
) -> AxResult<isize> {
    check_sigset_size(sigsetsize)?;

    let sig = make_queue_signal_info(tgid, signo, sig)?;
    send_signal_to_process(tgid, sig)?;
    Ok(0)
}

pub fn sys_rt_tgsigqueueinfo(
    tgid: Pid,
    tid: Pid,
    signo: u32,
    sig: *const SignalInfo,
    sigsetsize: usize,
) -> AxResult<isize> {
    check_sigset_size(sigsetsize)?;

    let sig = make_queue_signal_info(tgid, signo, sig)?;
    send_signal_to_thread(Some(tgid), tid, sig)?;
    Ok(0)
}

pub fn sys_rt_sigreturn(uctx: &mut UserContext) -> AxResult<isize> {
    block_next_signal();
    current().as_thread().signal.restore(uctx);
    Ok(uctx.retval() as isize)
}

pub fn sys_rt_sigtimedwait(
    uctx: &mut UserContext,
    set: *const SignalSet,
    info: *mut siginfo,
    timeout: *const timespec,
    sigsetsize: usize,
) -> AxResult<isize> {
    check_sigset_size(sigsetsize)?;

    let set = unsafe { set.vm_read_uninit()?.assume_init() };

    let timeout = if let Some(ts) = timeout.nullable() {
        let ts = unsafe { ts.vm_read_uninit()?.assume_init() };
        Some(ts.try_into_time_value()?)
    } else {
        None
    };

    debug!("sys_rt_sigtimedwait => set = {set:?}, timeout = {timeout:?}");

    let curr = current();
    let thr = curr.as_thread();
    let signal = &thr.signal;

    let old_blocked = signal.blocked();
    signal.set_blocked(old_blocked & !set);

    uctx.set_retval(-LinuxError::EINTR.code() as usize);
    let fut = poll_fn(|cx| {
        if thr.exec_exit_requested() {
            signal.set_blocked(old_blocked);
            Poll::Ready(None)
        } else if let Some(sig) = signal.dequeue_signal(&set) {
            signal.set_blocked(old_blocked);
            Poll::Ready(Some(sig))
        } else if check_signals(thr, uctx, Some(old_blocked)) {
            signal.set_blocked(old_blocked);
            Poll::Ready(None)
        } else {
            if curr.poll_interrupt(cx).is_ready() {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        }
    });

    let Ok(sig) = block_on(future::timeout(timeout, fut)) else {
        // Timeout
        signal.set_blocked(old_blocked);
        return Err(AxError::WouldBlock);
    };
    let Some(sig) = sig else {
        // Interrupted by a signal handler.
        return Err(AxError::Interrupted);
    };

    if let Some(info) = info.nullable() {
        info.vm_write(sig.0)?;
    }

    Ok(sig.signo() as _)
}

pub fn sys_rt_sigsuspend(
    uctx: &mut UserContext,
    set: *const SignalSet,
    sigsetsize: usize,
) -> AxResult<isize> {
    check_sigset_size(sigsetsize)?;

    let curr = current();
    let thr = curr.as_thread();

    let set = unsafe { set.vm_read_uninit()?.assume_init() };
    let old_blocked = thr.signal.set_blocked(set);

    // sigsuspend always returns -EINTR when a signal is caught
    // We set this in uctx before check_signals so it's saved in SignalFrame
    uctx.set_retval(-LinuxError::EINTR.code() as usize);

    block_on(poll_fn(|cx| {
        if thr.exec_exit_requested() {
            return Poll::Ready(());
        }
        if check_signals(thr, uctx, Some(old_blocked)) {
            return Poll::Ready(());
        }
        if curr.poll_interrupt(cx).is_ready() {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }));

    // sigsuspend always returns -EINTR
    Err(AxError::Interrupted)
}

pub fn sys_pause(uctx: &mut UserContext) -> AxResult<isize> {
    let curr = current();
    let thr = curr.as_thread();

    // pause always returns -EINTR when a signal is caught.
    uctx.set_retval(-LinuxError::EINTR.code() as usize);

    block_on(poll_fn(|cx| {
        if thr.exec_exit_requested() {
            return Poll::Ready(());
        }
        if check_signals(thr, uctx, None) {
            return Poll::Ready(());
        }
        if curr.poll_interrupt(cx).is_ready() {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }));

    Err(AxError::Interrupted)
}

pub fn sys_sigaltstack(ss: *const SignalStack, old_ss: *mut SignalStack) -> AxResult<isize> {
    let curr = current();
    let sig = &curr.as_thread().signal;

    if let Some(old_ss) = old_ss.nullable() {
        old_ss.vm_write(sig.stack())?;
    }

    if let Some(ss) = ss.nullable() {
        let ss = unsafe { ss.vm_read_uninit()?.assume_init() };
        const SUPPORTED_SS_FLAGS: u32 = SS_DISABLE | SS_AUTODISARM;
        if ss.flags & !SUPPORTED_SS_FLAGS != 0 {
            return Err(AxError::InvalidInput);
        }
        if ss.flags & SS_DISABLE == 0 && ss.size < MINSIGSTKSZ as usize {
            return Err(AxError::NoMemory);
        }
        sig.set_stack(ss);
    }
    Ok(0)
}
