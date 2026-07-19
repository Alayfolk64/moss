use alloc::vec::Vec;
use core::{future::poll_fn, task::Poll};

use axerrno::{AxError, AxResult, LinuxError};
use axhal::{time::TimeValue, uspace::UserContext};
use axpoll::IoEvents;
use axtask::{
    current,
    future::{self, block_on, poll_io},
};
use linux_raw_sys::general::{POLLNVAL, pollfd, timespec};
use starry_signal::SignalSet;
use starry_vm::{vm_load_any, vm_write_slice};

use super::{FdPollSet, poll_network_once};
use crate::{
    file::get_file_like,
    mm::{UserConstPtr, UserPtr, nullable},
    syscall::signal::check_sigset_size,
    task::{AsThread, check_signals, with_blocked_signals},
    time::TimeValueLike,
};

fn do_poll(
    poll_fds: &mut [pollfd],
    timeout: Option<TimeValue>,
    sigmask: Option<SignalSet>,
) -> AxResult<isize> {
    debug!("do_poll fds={poll_fds:?} timeout={timeout:?}");

    let mut res = 0isize;
    let mut fds = Vec::with_capacity(poll_fds.len());
    let mut revents = Vec::with_capacity(poll_fds.len());
    for fd in poll_fds.iter_mut() {
        if fd.fd == -1 {
            // Skip -1
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
    if res > 0 {
        return Ok(res);
    }
    let fds = FdPollSet(fds);

    with_blocked_signals(sigmask, || {
        match block_on(future::timeout(
            timeout,
            poll_io(&fds, IoEvents::empty(), false, || {
                poll_network_once();
                let mut res = 0usize;
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
            }),
        )) {
            Ok(r) => r,
            Err(_) => Ok(0),
        }
    })
}

fn poll_user_fds(
    user_fds: UserPtr<pollfd>,
    nfds: usize,
    timeout: Option<TimeValue>,
    sigmask: Option<SignalSet>,
) -> AxResult<isize> {
    let user_fds = user_fds.address().as_mut_ptr_of::<pollfd>();

    // pollfd contains only integer fields, so every bit pattern is valid.
    let mut poll_fds = unsafe { vm_load_any(user_fds, nfds)? };
    let result = do_poll(&mut poll_fds, timeout, sigmask);

    if result.is_ok() {
        vm_write_slice(user_fds, &poll_fds)?;
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
            if check_signals(thr, uctx, old_blocked) {
                return Poll::Ready(());
            }
            if curr.poll_interrupt(cx).is_ready() {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        }),
    ));

    match interrupted {
        Ok(()) => Err(AxError::Interrupted),
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
        poll_user_fds(fds, nfds, timeout, sigmask)
    }
}
