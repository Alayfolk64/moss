use axerrno::{AxError, AxResult};
use axhal::time::{TimeValue, monotonic_time, monotonic_time_nanos, nanos_to_ticks, wall_time};
use axtask::current;
use linux_raw_sys::general::{
    __kernel_clockid_t, CLOCK_BOOTTIME, CLOCK_MONOTONIC, CLOCK_MONOTONIC_COARSE,
    CLOCK_MONOTONIC_RAW, CLOCK_PROCESS_CPUTIME_ID, CLOCK_REALTIME, CLOCK_REALTIME_COARSE,
    CLOCK_THREAD_CPUTIME_ID, itimerval, timespec, timeval,
};
use starry_vm::{VmMutPtr, VmPtr};

use crate::{
    task::{AsThread, ITimerType},
    time::TimeValueLike,
};

const ADJ_OFFSET: u32 = 0x0001;
const ADJ_FREQUENCY: u32 = 0x0002;
const ADJ_MAXERROR: u32 = 0x0004;
const ADJ_ESTERROR: u32 = 0x0008;
const ADJ_STATUS: u32 = 0x0010;
const ADJ_TIMECONST: u32 = 0x0020;
const ADJ_TAI: u32 = 0x0080;
const ADJ_SETOFFSET: u32 = 0x0100;
const ADJ_MICRO: u32 = 0x1000;
const ADJ_NANO: u32 = 0x2000;
const ADJ_TICK: u32 = 0x4000;
const ADJ_OFFSET_SINGLESHOT: u32 = 0x8001;
const ADJ_OFFSET_SS_READ: u32 = 0xa001;
const ADJ_OFFSET_SINGLESHOT_FLAG: u32 = ADJ_OFFSET_SINGLESHOT & !ADJ_OFFSET;
const ADJ_READ_ONLY_MODES: u32 = ADJ_OFFSET_SS_READ;
const ADJ_VALID_MODES: u32 = ADJ_OFFSET
    | ADJ_FREQUENCY
    | ADJ_MAXERROR
    | ADJ_ESTERROR
    | ADJ_STATUS
    | ADJ_TIMECONST
    | ADJ_TAI
    | ADJ_SETOFFSET
    | ADJ_MICRO
    | ADJ_NANO
    | ADJ_TICK
    | ADJ_OFFSET_SINGLESHOT
    | ADJ_OFFSET_SS_READ;

const TIME_OK: isize = 0;
const MIN_TICK: isize = 9000;
const MAX_TICK: isize = 11000;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timex {
    modes: u32,
    _pad0: i32,
    offset: isize,
    freq: isize,
    maxerror: isize,
    esterror: isize,
    status: i32,
    _pad1: i32,
    constant: isize,
    precision: isize,
    tolerance: isize,
    time: timeval,
    tick: isize,
    ppsfreq: isize,
    jitter: isize,
    shift: i32,
    _pad2: i32,
    stabil: isize,
    jitcnt: isize,
    calcnt: isize,
    errcnt: isize,
    stbcnt: isize,
    tai: i32,
    _pad3: [i32; 11],
}

impl Timex {
    fn snapshot(modes: u32) -> Self {
        Self {
            modes,
            _pad0: 0,
            offset: 0,
            freq: 0,
            maxerror: 0,
            esterror: 0,
            status: 0,
            _pad1: 0,
            constant: 0,
            precision: 1,
            tolerance: 0,
            time: timeval::from_time_value(wall_time()),
            tick: 10_000,
            ppsfreq: 0,
            jitter: 0,
            shift: 0,
            _pad2: 0,
            stabil: 0,
            jitcnt: 0,
            calcnt: 0,
            errcnt: 0,
            stbcnt: 0,
            tai: 0,
            _pad3: [0; 11],
        }
    }
}

pub fn sys_clock_gettime(clock_id: __kernel_clockid_t, ts: *mut timespec) -> AxResult<isize> {
    let now = match clock_id as u32 {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => wall_time(),
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME => {
            monotonic_time()
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            let (utime, stime) = current().as_thread().time.borrow().output();
            utime + stime
        }
        _ => {
            warn!("Called sys_clock_gettime for unsupported clock {clock_id}");
            wall_time()
            // return Err(AxError::EINVAL);
        }
    };
    ts.vm_write(timespec::from_time_value(now))?;
    Ok(0)
}

pub fn sys_adjtimex(buf: *mut Timex) -> AxResult<isize> {
    let curr = unsafe { buf.vm_read_uninit()?.assume_init() };
    if curr.modes & !ADJ_VALID_MODES != 0 || curr.modes == ADJ_OFFSET_SINGLESHOT_FLAG {
        return Err(AxError::InvalidInput);
    }
    if curr.modes & ADJ_TICK != 0 && !(MIN_TICK..=MAX_TICK).contains(&curr.tick) {
        return Err(AxError::InvalidInput);
    }
    if curr.modes & !ADJ_READ_ONLY_MODES != 0 && current().as_thread().proc_data.euid() != 0 {
        return Err(AxError::OperationNotPermitted);
    }
    buf.vm_write(Timex::snapshot(curr.modes))?;
    Ok(TIME_OK)
}

pub fn sys_clock_adjtime(clock_id: __kernel_clockid_t, buf: *mut Timex) -> AxResult<isize> {
    if clock_id as u32 != CLOCK_REALTIME {
        return Err(AxError::InvalidInput);
    }
    sys_adjtimex(buf)
}

pub fn sys_gettimeofday(ts: *mut timeval) -> AxResult<isize> {
    ts.vm_write(timeval::from_time_value(wall_time()))?;
    Ok(0)
}

pub fn sys_clock_getres(clock_id: __kernel_clockid_t, res: *mut timespec) -> AxResult<isize> {
    if clock_id as u32 != CLOCK_MONOTONIC && clock_id as u32 != CLOCK_REALTIME {
        warn!("Called sys_clock_getres for unsupported clock {clock_id}");
    }
    if let Some(res) = res.nullable() {
        res.vm_write(timespec::from_time_value(TimeValue::from_micros(1)))?;
    }
    Ok(0)
}

#[repr(C)]
pub struct Tms {
    /// user time
    tms_utime: usize,
    /// system time
    tms_stime: usize,
    /// user time of children
    tms_cutime: usize,
    /// system time of children
    tms_cstime: usize,
}

pub fn sys_times(tms: *mut Tms) -> AxResult<isize> {
    let (utime, stime) = current().as_thread().time.borrow().output();
    let utime = utime.as_micros() as usize;
    let stime = stime.as_micros() as usize;
    tms.vm_write(Tms {
        tms_utime: utime,
        tms_stime: stime,
        tms_cutime: utime,
        tms_cstime: stime,
    })?;
    Ok(nanos_to_ticks(monotonic_time_nanos()) as _)
}

pub fn sys_getitimer(which: i32, value: *mut itimerval) -> AxResult<isize> {
    let ty = ITimerType::from_repr(which).ok_or(AxError::InvalidInput)?;
    let (it_interval, it_value) = current().as_thread().time.borrow().get_itimer(ty);

    value.vm_write(itimerval {
        it_interval: timeval::from_time_value(it_interval),
        it_value: timeval::from_time_value(it_value),
    })?;
    Ok(0)
}

pub fn sys_setitimer(
    which: i32,
    new_value: *const itimerval,
    old_value: *mut itimerval,
) -> AxResult<isize> {
    let ty = ITimerType::from_repr(which).ok_or(AxError::InvalidInput)?;
    let curr = current();

    let (interval, remained) = match new_value.nullable() {
        Some(new_value) => {
            // FIXME: AnyBitPattern
            let new_value = unsafe { new_value.vm_read_uninit()?.assume_init() };
            (
                new_value.it_interval.try_into_time_value()?.as_nanos() as usize,
                new_value.it_value.try_into_time_value()?.as_nanos() as usize,
            )
        }
        None => (0, 0),
    };

    debug!("sys_setitimer <= type: {ty:?}, interval: {interval:?}, remained: {remained:?}");

    let old = curr
        .as_thread()
        .time
        .borrow_mut()
        .set_itimer(ty, interval, remained);

    if let Some(old_value) = old_value.nullable() {
        old_value.vm_write(itimerval {
            it_interval: timeval::from_time_value(old.0),
            it_value: timeval::from_time_value(old.1),
        })?;
    }
    Ok(0)
}
