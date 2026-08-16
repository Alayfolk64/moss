use core::{
    mem::{MaybeUninit, size_of},
    slice,
    time::Duration,
};

use axerrno::{AxError, AxResult};
use axpoll::IoEvents;
use axtask::future::{self, block_on, poll_io};
use bitflags::bitflags;
use linux_raw_sys::general::{
    EPOLL_CLOEXEC, EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLL_CTL_MOD, epoll_event, timespec,
};
use starry_signal::SignalSet;
use starry_vm::vm_read_slice;

use super::poll_network_once;
use crate::{
    file::{
        FileLike,
        epoll::{Epoll, EpollEvent, EpollFlags},
    },
    mm::{UserConstPtr, UserPtr, nullable},
    syscall::signal::check_sigset_size,
    task::with_blocked_signals,
    time::TimeValueLike,
};

bitflags! {
    /// Flags for the `epoll_create` syscall.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct EpollCreateFlags: u32 {
        const CLOEXEC = EPOLL_CLOEXEC;
    }
}

pub fn sys_epoll_create1(flags: u32) -> AxResult<isize> {
    let flags = EpollCreateFlags::from_bits(flags).ok_or(AxError::InvalidInput)?;
    debug!("sys_epoll_create1 <= flags: {flags:?}");
    Epoll::new()
        .add_to_fd_table(flags.contains(EpollCreateFlags::CLOEXEC))
        .map(|fd| fd as isize)
}

fn read_epoll_event(event: UserConstPtr<epoll_event>) -> AxResult<epoll_event> {
    let mut raw = MaybeUninit::<epoll_event>::uninit();
    let bytes = unsafe {
        slice::from_raw_parts_mut(
            raw.as_mut_ptr().cast::<MaybeUninit<u8>>(),
            size_of::<epoll_event>(),
        )
    };
    vm_read_slice(event.address().as_usize() as *const u8, bytes)?;
    Ok(unsafe { raw.assume_init() })
}

fn poll_epoll_events(epoll: &Epoll, events: &mut [epoll_event]) -> AxResult<isize> {
    match epoll.poll_events(events) {
        Ok(n) => Ok(n as _),
        Err(AxError::WouldBlock) => Ok(0),
        Err(err) => Err(err),
    }
}

pub fn sys_epoll_ctl(
    epfd: i32,
    op: u32,
    fd: i32,
    event: UserConstPtr<epoll_event>,
) -> AxResult<isize> {
    let epoll = Epoll::from_fd(epfd)?;
    debug!("sys_epoll_ctl <= epfd: {epfd}, op: {op}, fd: {fd}");

    let parse_event = || -> AxResult<(EpollEvent, EpollFlags)> {
        let event = read_epoll_event(event)?;
        let events = IoEvents::from_bits_truncate(event.events);
        let flags =
            EpollFlags::from_bits(event.events & !events.bits()).ok_or(AxError::InvalidInput)?;
        Ok((
            EpollEvent {
                events,
                user_data: event.data,
            },
            flags,
        ))
    };
    match op {
        EPOLL_CTL_ADD => {
            if fd == epfd {
                return Err(AxError::InvalidInput);
            }
            let (event, flags) = parse_event()?;
            epoll.add(fd, event, flags)?;
        }
        EPOLL_CTL_MOD => {
            let (event, flags) = parse_event()?;
            epoll.modify(fd, event, flags)?;
        }
        EPOLL_CTL_DEL => {
            epoll.delete(fd)?;
        }
        _ => return Err(AxError::InvalidInput),
    }
    Ok(0)
}

fn do_epoll_wait(
    epfd: i32,
    events: UserPtr<epoll_event>,
    maxevents: i32,
    timeout: Option<Duration>,
    sigmask: UserConstPtr<SignalSet>,
    sigsetsize: usize,
) -> AxResult<isize> {
    debug!("sys_epoll_wait <= epfd: {epfd}, maxevents: {maxevents}, timeout: {timeout:?}");

    if maxevents <= 0 {
        return Err(AxError::InvalidInput);
    }
    check_sigset_size(sigsetsize)?;

    if timeout == Some(Duration::ZERO) && sigmask.is_null() {
        let epoll = Epoll::from_fd(epfd)?;
        if !epoll.has_ready_events() {
            return Ok(0);
        }
        return poll_epoll_events(&epoll, events.get_as_mut_slice(maxevents as usize)?);
    }

    let epoll = Epoll::from_fd(epfd)?;
    let sigmask = nullable!(sigmask.get_as_ref())?.copied();

    let events = events.get_as_mut_slice(maxevents as usize)?;

    let result = with_blocked_signals(sigmask, || {
        if timeout == Some(Duration::ZERO) {
            return poll_epoll_events(&epoll, events);
        }

        match block_on(future::timeout(
            timeout,
            poll_io(epoll.as_ref(), IoEvents::IN, false, || {
                poll_network_once();
                epoll.poll_events(events)
            }),
        )) {
            // epoll wait timeouts are reported as a zero event count, never
            // as ETIMEDOUT. Preserve that ABI even if a nested poll future
            // surfaces its own elapsed deadline.
            Ok(Err(AxError::TimedOut)) => Ok(0),
            Ok(r) => r.map(|n| n as _),
            Err(_) => Ok(0),
        }
    });

    // Keep the Linux ABI at the syscall boundary as well as at the nested
    // timeout boundary above. A poll implementation may surface ETIMEDOUT
    // directly while unwinding its registered wait state.
    match result {
        Err(AxError::TimedOut) => Ok(0),
        result => result,
    }
}

pub fn sys_epoll_pwait(
    epfd: i32,
    events: UserPtr<epoll_event>,
    maxevents: i32,
    timeout: i32,
    sigmask: UserConstPtr<SignalSet>,
    sigsetsize: usize,
) -> AxResult<isize> {
    let timeout = match timeout {
        -1 => None,
        t if t >= 0 => Some(Duration::from_millis(t as u64)),
        _ => return Err(AxError::InvalidInput),
    };
    do_epoll_wait(epfd, events, maxevents, timeout, sigmask, sigsetsize)
}

pub fn sys_epoll_pwait2(
    epfd: i32,
    events: UserPtr<epoll_event>,
    maxevents: i32,
    timeout: UserConstPtr<timespec>,
    sigmask: UserConstPtr<SignalSet>,
    sigsetsize: usize,
) -> AxResult<isize> {
    let timeout = nullable!(timeout.get_as_ref())?
        .map(|ts| ts.try_into_time_value())
        .transpose()?;
    do_epoll_wait(epfd, events, maxevents, timeout, sigmask, sigsetsize)
}
