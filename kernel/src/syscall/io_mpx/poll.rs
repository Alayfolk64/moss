use alloc::vec::Vec;
use core::{
    future::poll_fn,
    mem::{offset_of, size_of},
    slice,
    task::Poll,
};

use axerrno::{AxError, AxResult, LinuxError};
use axhal::{time::TimeValue, uspace::UserContext};
use axpoll::{IoEvents, Pollable};
use axtask::{
    current,
    future::{self, block_on, poll_io},
};
use linux_raw_sys::general::{POLLNVAL, RLIMIT_NOFILE, pollfd, timespec};
use starry_signal::SignalSet;
use starry_vm::{vm_load_any, vm_write_slice};

use super::{FdPollSet, poll_network_once};
use crate::{
    file::get_file_like,
    mm::{UserConstPtr, UserPtr, nullable},
    syscall::{SYSCALL_RESTART, signal::check_sigset_size},
    task::{AsThread, SignalCheckResult, check_signals_result},
    time::TimeValueLike,
};

fn poll_fds_once(
    fds: &FdPollSet,
    revents: &mut [&mut i16],
    initially_ready: isize,
) -> AxResult<isize> {
    poll_network_once();
    let mut res = initially_ready as usize;
    for ((fd, events), revents) in fds.0.iter().zip(revents.iter_mut()) {
        let mut result = fd.poll();
        if result.contains(IoEvents::IN) {
            result |= IoEvents::RDNORM;
        }
        if result.contains(IoEvents::OUT) {
            result |= IoEvents::WRNORM;
        }
        result &= *events;

        **revents = result.bits() as _;
        if **revents != 0 {
            res += 1;
        }
    }
    if res > 0 {
        Ok(res as _)
    } else {
        Err(AxError::WouldBlock)
    }
}

fn poll_with_signal_mask(
    uctx: &mut UserContext,
    fds: &FdPollSet,
    revents: &mut [&mut i16],
    initially_ready: isize,
    timeout: Option<TimeValue>,
    sigmask: SignalSet,
) -> AxResult<isize> {
    let curr = current();
    let thr = curr.as_thread();
    let old_blocked = thr.signal.set_blocked(sigmask);

    // If a handler is installed below, this value is captured in its signal
    // frame.  SYSCALL_RESTART then prevents the dispatcher from overwriting
    // the handler arguments in the live UserContext.
    uctx.set_retval(-LinuxError::EINTR.code() as usize);
    let result = block_on(future::timeout(
        timeout,
        poll_fn(|cx| {
            if thr.pending_exit() || thr.exec_exit_requested() {
                return Poll::Ready(Err(AxError::Interrupted));
            }

            match poll_fds_once(fds, revents, initially_ready) {
                Ok(ready) => return Poll::Ready(Ok(ready)),
                Err(AxError::WouldBlock) => {}
                Err(err) => return Poll::Ready(Err(err)),
            }

            fds.register(cx, IoEvents::empty());
            match poll_fds_once(fds, revents, initially_ready) {
                Ok(ready) => return Poll::Ready(Ok(ready)),
                Err(AxError::WouldBlock) => {}
                Err(err) => return Poll::Ready(Err(err)),
            }

            match check_signals_result(thr, uctx, Some(old_blocked)) {
                SignalCheckResult::None => {}
                SignalCheckResult::Handler => return Poll::Ready(Ok(SYSCALL_RESTART)),
                SignalCheckResult::Other => {
                    return Poll::Ready(Err(AxError::Interrupted));
                }
            }

            if curr.poll_interrupt(cx).is_ready() {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        }),
    ));

    match result {
        Ok(Ok(SYSCALL_RESTART)) => Ok(SYSCALL_RESTART),
        Ok(result) => {
            thr.signal.set_blocked(old_blocked);
            result
        }
        Err(_) => {
            thr.signal.set_blocked(old_blocked);
            Ok(0)
        }
    }
}

fn do_poll(
    poll_fds: &mut [pollfd],
    timeout: Option<TimeValue>,
    signal_wait: Option<(&mut UserContext, SignalSet)>,
) -> AxResult<isize> {
    debug!("do_poll fds={poll_fds:?} timeout={timeout:?}");

    let mut res = 0isize;
    let mut fds = Vec::with_capacity(poll_fds.len());
    let mut revents = Vec::with_capacity(poll_fds.len());
    for fd in poll_fds.iter_mut() {
        fd.revents = 0;
        if fd.fd < 0 {
            // Linux ignores every negative descriptor.
            continue;
        }
        match get_file_like(fd.fd) {
            Ok(f) => {
                fds.push((
                    f,
                    IoEvents::from_bits(fd.events as _).ok_or(AxError::InvalidInput)?
                        | IoEvents::ALWAYS_POLL,
                ));
                revents.push(&mut fd.revents);
            }
            Err(_) => {
                // If the fd is invalid, set revents to POLLNVAL
                fd.revents = POLLNVAL as _;
                res += 1;
            }
        }
    }
    let fds = FdPollSet(fds);

    if let Some((uctx, sigmask)) = signal_wait {
        return poll_with_signal_mask(uctx, &fds, &mut revents, res, timeout, sigmask);
    }

    match block_on(future::timeout(
        timeout,
        poll_io(&fds, IoEvents::empty(), false, || {
            poll_fds_once(&fds, &mut revents, res)
        }),
    )) {
        Ok(r) => r,
        Err(_) => Ok(0),
    }
}

fn poll_user_fds(
    user_fds: UserPtr<pollfd>,
    nfds: usize,
    timeout: Option<TimeValue>,
    signal_wait: Option<(&mut UserContext, SignalSet)>,
) -> AxResult<isize> {
    let max_nfds = current().as_thread().proc_data.rlim.read()[RLIMIT_NOFILE].current;
    if nfds as u64 > max_nfds {
        return Err(AxError::InvalidInput);
    }

    let user_fds = user_fds.address().as_mut_ptr_of::<pollfd>();

    // pollfd contains only integer fields, so every bit pattern is valid.
    let mut poll_fds = unsafe { vm_load_any(user_fds, nfds)? };
    let result = do_poll(&mut poll_fds, timeout, signal_wait);

    if matches!(&result, Ok(ret) if *ret != SYSCALL_RESTART) {
        // `fd` and `events` are user inputs.  Publish only `revents`, so a
        // concurrent user update to either input is not overwritten by the
        // snapshot taken before the blocking wait.
        for (index, poll_fd) in poll_fds.iter().enumerate() {
            let revents_addr = user_fds
                .addr()
                .checked_add(index * size_of::<pollfd>())
                .and_then(|addr| addr.checked_add(offset_of!(pollfd, revents)))
                .ok_or(AxError::BadAddress)?;
            vm_write_slice(revents_addr as *mut _, slice::from_ref(&poll_fd.revents))?;
        }
    }
    result
}

pub(super) fn wait_for_signal_or_timeout(
    uctx: &mut UserContext,
    timeout: Option<TimeValue>,
    sigmask: Option<SignalSet>,
) -> AxResult<isize> {
    let curr = current();
    let thr = curr.as_thread();
    let old_blocked = sigmask.map(|set| thr.signal.set_blocked(set));

    uctx.set_retval(-LinuxError::EINTR.code() as usize);
    let interrupted = block_on(future::timeout(
        timeout,
        poll_fn(|cx| {
            if thr.pending_exit() || thr.exec_exit_requested() {
                return Poll::Ready(SignalCheckResult::Other);
            }
            match check_signals_result(thr, uctx, old_blocked) {
                SignalCheckResult::None => {}
                result => return Poll::Ready(result),
            }
            if curr.poll_interrupt(cx).is_ready() {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        }),
    ));

    match interrupted {
        Ok(SignalCheckResult::Handler) => Ok(SYSCALL_RESTART),
        Ok(SignalCheckResult::Other) => {
            if let Some(old_blocked) = old_blocked {
                thr.signal.set_blocked(old_blocked);
            }
            Err(AxError::Interrupted)
        }
        Ok(SignalCheckResult::None) => unreachable!(),
        Err(_) => {
            if let Some(old_blocked) = old_blocked {
                thr.signal.set_blocked(old_blocked);
            }
            Ok(0)
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub fn sys_poll(fds: UserPtr<pollfd>, nfds: u32, timeout: i32) -> AxResult<isize> {
    let timeout = if timeout < 0 {
        None
    } else {
        Some(TimeValue::from_millis(timeout as u64))
    };
    poll_user_fds(fds, nfds as usize, timeout, None)
}

pub fn sys_ppoll(
    uctx: &mut UserContext,
    fds: UserPtr<pollfd>,
    nfds: i32,
    timeout: UserConstPtr<timespec>,
    sigmask: UserConstPtr<SignalSet>,
    sigsetsize: usize,
) -> AxResult<isize> {
    let nfds = nfds.try_into().map_err(|_| AxError::InvalidInput)?;
    let timeout = nullable!(timeout.get_as_ref())?
        .map(|ts| ts.try_into_time_value())
        .transpose()?;
    let sigmask = nullable!(sigmask.get_as_ref())?;
    if sigmask.is_some() {
        check_sigset_size(sigsetsize)?;
    }
    let sigmask = sigmask.copied();

    if nfds == 0 {
        wait_for_signal_or_timeout(uctx, timeout, sigmask)
    } else {
        let signal_wait = sigmask.map(|sigmask| (uctx, sigmask));
        poll_user_fds(fds, nfds, timeout, signal_wait)
    }
}
