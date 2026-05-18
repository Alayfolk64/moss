use alloc::{collections::BTreeMap, format};

use axerrno::{AxError, AxResult};
use axhal::time::{TimeValue, monotonic_time, monotonic_time_nanos, nanos_to_ticks, wall_time};
use axtask::{current, yield_now};
use lazy_static::lazy_static;
use linux_raw_sys::general::{
    __kernel_clockid_t, CLOCK_BOOTTIME, CLOCK_BOOTTIME_ALARM, CLOCK_MONOTONIC,
    CLOCK_MONOTONIC_COARSE, CLOCK_MONOTONIC_RAW, CLOCK_PROCESS_CPUTIME_ID, CLOCK_REALTIME,
    CLOCK_REALTIME_ALARM, CLOCK_REALTIME_COARSE, CLOCK_TAI, CLOCK_THREAD_CPUTIME_ID, SIGEV_NONE,
    SIGEV_SIGNAL, SIGEV_THREAD, SIGEV_THREAD_ID, TFD_CLOEXEC, TFD_NONBLOCK, TFD_TIMER_ABSTIME,
    TFD_TIMER_CANCEL_ON_SET, TIMER_ABSTIME, itimerspec, itimerval, sigevent, timespec, timeval,
};
use spin::Mutex;
use starry_process::Pid;
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr};

use crate::{
    file::{FileLike, TimerFd, add_file_like, timerfd::KernelTimerSpec},
    task::{AsThread, ITimerType, get_process_data, get_task, send_signal_to_process},
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
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM | CLOCK_TAI => wall_time(),
        CLOCK_MONOTONIC
        | CLOCK_MONOTONIC_RAW
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_BOOTTIME
        | CLOCK_BOOTTIME_ALARM => monotonic_time(),
        CLOCK_PROCESS_CPUTIME_ID => {
            process_cpu_time(current().as_thread().proc_data.proc.pid()).unwrap_or(TimeValue::ZERO)
        }
        CLOCK_THREAD_CPUTIME_ID => {
            let (utime, stime) = current().as_thread().time.borrow().output();
            time_value_add(utime, stime)
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
    let curr = current();
    let (utime, stime) = curr.as_thread().time.borrow().output();
    let utime = to_user_hz_ticks(utime.as_micros()) as usize;
    let stime = to_user_hz_ticks(stime.as_micros()) as usize;
    let child_time = curr.as_thread().proc_data.child_cpu_time.lock();
    tms.vm_write(Tms {
        tms_utime: utime,
        tms_stime: stime,
        tms_cutime: child_time.0 as usize,
        tms_cstime: child_time.1 as usize,
    })?;
    Ok(nanos_to_ticks(monotonic_time_nanos()) as _)
}

fn to_user_hz_ticks(micros: u128) -> u64 {
    const MICROS_PER_TICK: u128 = 10_000;
    if micros == 0 {
        0
    } else {
        ((micros - 1) / MICROS_PER_TICK + 1) as u64
    }
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

    let old = loop {
        if let Ok(mut time) = curr.as_thread().time.try_borrow_mut() {
            break time.set_itimer(ty, interval, remained);
        }
        yield_now();
    };

    if let Some(old_value) = old_value.nullable() {
        old_value.vm_write(itimerval {
            it_interval: timeval::from_time_value(old.0),
            it_value: timeval::from_time_value(old.1),
        })?;
    }
    Ok(0)
}

pub fn sys_timer_create(
    clock_id: __kernel_clockid_t,
    evp: *const sigevent,
    timerid: *mut i32,
) -> AxResult<isize> {
    let clock_id = clock_id as u32;
    if !valid_posix_timer_clock(clock_id) {
        return Err(AxError::InvalidInput);
    }
    let signo = parse_sigevent(evp)?;
    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    let tid = curr.id().as_u64() as Pid;
    let id = POSIX_TIMERS.lock().create(pid, tid, clock_id, signo);
    if let Err(err) = timerid.vm_write(id) {
        POSIX_TIMERS.lock().delete(pid, id);
        return Err(err.into());
    }
    Ok(0)
}

pub fn sys_timer_delete(timerid: i32) -> AxResult<isize> {
    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    POSIX_TIMERS
        .lock()
        .delete(pid, timerid)
        .ok_or(AxError::InvalidInput)?;
    Ok(0)
}

pub fn sys_timer_getoverrun(timerid: i32) -> AxResult<isize> {
    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    let overrun = POSIX_TIMERS
        .lock()
        .get(pid, timerid)
        .map(|timer| timer.overrun)
        .ok_or(AxError::InvalidInput)?;
    Ok(overrun as _)
}

pub fn sys_timer_gettime(timerid: i32, curr_value: *mut itimerspec) -> AxResult<isize> {
    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    let spec = POSIX_TIMERS
        .lock()
        .get(pid, timerid)
        .ok_or(AxError::InvalidInput)
        .and_then(PosixTimer::gettime)?;
    write_itimerspec(curr_value, spec)?;
    Ok(0)
}

pub fn sys_timer_settime(
    timerid: i32,
    flags: u32,
    new_value: *const itimerspec,
    old_value: *mut itimerspec,
) -> AxResult<isize> {
    if flags & !TIMER_ABSTIME != 0 {
        return Err(AxError::InvalidInput);
    }
    let Some(new_value) = new_value.nullable() else {
        return Err(AxError::InvalidInput);
    };
    let new_spec = read_itimerspec(new_value)?;

    let curr = current();
    let pid = curr.as_thread().proc_data.proc.pid();
    let (old, arm) =
        POSIX_TIMERS
            .lock()
            .settime(pid, timerid, new_spec, flags & TIMER_ABSTIME != 0)?;
    if let Some(old_value) = old_value.nullable() {
        write_itimerspec(old_value, old)?;
    }
    if let Some((deadline, generation)) = arm {
        spawn_posix_timer(pid, timerid, deadline, generation);
    }
    Ok(0)
}

pub fn sys_timerfd_create(clock_id: __kernel_clockid_t, flags: u32) -> AxResult<isize> {
    let clock_id = clock_id as u32;
    if !valid_timerfd_clock(clock_id) || flags & !(TFD_CLOEXEC | TFD_NONBLOCK) != 0 {
        return Err(AxError::InvalidInput);
    }

    let timerfd = TimerFd::new(clock_id);
    timerfd.set_nonblocking(flags & TFD_NONBLOCK != 0)?;
    add_file_like(timerfd as _, flags & TFD_CLOEXEC != 0).map(|fd| fd as _)
}

pub fn sys_timerfd_gettime(fd: i32, curr_value: *mut itimerspec) -> AxResult<isize> {
    let timerfd = TimerFd::from_fd(fd)?;
    write_itimerspec(curr_value, timerfd.gettime())?;
    Ok(0)
}

pub fn sys_timerfd_settime(
    fd: i32,
    flags: u32,
    new_value: *const itimerspec,
    old_value: *mut itimerspec,
) -> AxResult<isize> {
    if flags & !(TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET) != 0 {
        return Err(AxError::InvalidInput);
    }
    let timerfd = TimerFd::from_fd(fd)?;
    let new_spec = read_itimerspec(new_value)?;
    let old = timerfd.settime(new_spec, flags & TFD_TIMER_ABSTIME != 0);
    if let Some(old_value) = old_value.nullable() {
        write_itimerspec(old_value, old)?;
    }
    Ok(0)
}

fn parse_sigevent(evp: *const sigevent) -> AxResult<Option<Signo>> {
    let Some(evp) = evp.nullable() else {
        return Ok(Some(Signo::SIGALRM));
    };
    let evp = unsafe { evp.vm_read_uninit()?.assume_init() };
    match evp.sigev_notify as u32 {
        SIGEV_NONE | SIGEV_THREAD => Ok(None),
        SIGEV_SIGNAL | SIGEV_THREAD_ID => Signo::from_repr(evp.sigev_signo as u8)
            .map(Some)
            .ok_or(AxError::InvalidInput),
        _ => Err(AxError::InvalidInput),
    }
}

fn read_itimerspec(ptr: *const itimerspec) -> AxResult<KernelTimerSpec> {
    let spec = unsafe { ptr.vm_read_uninit()?.assume_init() };
    Ok(KernelTimerSpec {
        interval: spec.it_interval.try_into_time_value()?,
        value: spec.it_value.try_into_time_value()?,
    })
}

fn write_itimerspec(ptr: *mut itimerspec, spec: KernelTimerSpec) -> AxResult<()> {
    Ok(ptr.vm_write(itimerspec {
        it_interval: timespec::from_time_value(spec.interval),
        it_value: timespec::from_time_value(spec.value),
    })?)
}

#[derive(Clone, Copy)]
struct PosixTimer {
    owner_pid: Pid,
    owner_tid: Pid,
    clock_id: u32,
    signo: Option<Signo>,
    interval: TimeValue,
    deadline: Option<TimeValue>,
    generation: u64,
    overrun: i32,
}

impl PosixTimer {
    fn gettime(&self) -> AxResult<KernelTimerSpec> {
        let now = posix_timer_clock_now(self.clock_id, self.owner_pid, self.owner_tid)
            .ok_or(AxError::NoSuchProcess)?;
        Ok(KernelTimerSpec {
            interval: self.interval,
            value: self
                .deadline
                .map(|deadline| deadline.saturating_sub(now))
                .unwrap_or(TimeValue::ZERO),
        })
    }
}

#[derive(Default)]
struct PosixTimerTable {
    next_id: BTreeMap<Pid, i32>,
    timers: BTreeMap<(Pid, i32), PosixTimer>,
}

impl PosixTimerTable {
    fn create(&mut self, pid: Pid, tid: Pid, clock_id: u32, signo: Option<Signo>) -> i32 {
        let next = self.next_id.entry(pid).or_default();
        let id = *next;
        *next += 1;
        self.timers.insert(
            (pid, id),
            PosixTimer {
                owner_pid: pid,
                owner_tid: tid,
                clock_id,
                signo,
                interval: TimeValue::ZERO,
                deadline: None,
                generation: 0,
                overrun: 0,
            },
        );
        id
    }

    fn delete(&mut self, pid: Pid, id: i32) -> Option<()> {
        self.timers.remove(&(pid, id)).map(|_| ())
    }

    fn get(&self, pid: Pid, id: i32) -> Option<&PosixTimer> {
        self.timers.get(&(pid, id))
    }

    fn settime(
        &mut self,
        pid: Pid,
        id: i32,
        spec: KernelTimerSpec,
        abstime: bool,
    ) -> AxResult<(KernelTimerSpec, Option<(TimeValue, u64)>)> {
        let timer = self
            .timers
            .get_mut(&(pid, id))
            .ok_or(AxError::InvalidInput)?;
        let now = posix_timer_clock_now(timer.clock_id, timer.owner_pid, timer.owner_tid)
            .ok_or(AxError::NoSuchProcess)?;
        let old = timer.gettime()?;
        timer.generation = timer.generation.wrapping_add(1);
        timer.interval = spec.interval;
        timer.overrun = if abstime && !spec.interval.is_zero() && spec.value < now {
            i32::MAX
        } else {
            0
        };
        timer.deadline = if spec.disarmed() {
            None
        } else if abstime {
            Some(spec.value)
        } else {
            Some(now.checked_add(spec.value).unwrap_or(TimeValue::MAX))
        };
        Ok((
            old,
            timer.deadline.map(|deadline| (deadline, timer.generation)),
        ))
    }
}

lazy_static! {
    static ref POSIX_TIMERS: Mutex<PosixTimerTable> = Mutex::new(PosixTimerTable::default());
}

fn spawn_posix_timer(pid: Pid, timerid: i32, deadline: TimeValue, generation: u64) {
    axtask::spawn_raw(
        move || posix_timer_worker(pid, timerid, deadline, generation),
        format!("ltp_timer:{pid}:{timerid}"),
        axconfig::TASK_STACK_SIZE,
    );
}

fn posix_timer_worker(pid: Pid, timerid: i32, mut deadline: TimeValue, generation: u64) {
    loop {
        let Some(clock_id) = POSIX_TIMERS
            .lock()
            .get(pid, timerid)
            .filter(|timer| timer.generation == generation)
            .map(|timer| (timer.clock_id, timer.owner_pid, timer.owner_tid))
        else {
            return;
        };
        let (clock_id, owner_pid, owner_tid) = clock_id;

        loop {
            let Some(now) = posix_timer_clock_now(clock_id, owner_pid, owner_tid) else {
                return;
            };
            if now >= deadline {
                break;
            }
            if !POSIX_TIMERS
                .lock()
                .get(pid, timerid)
                .is_some_and(|timer| timer.generation == generation)
            {
                return;
            }
            yield_now();
        }

        let (signo, interval, next_deadline) = {
            let mut timers = POSIX_TIMERS.lock();
            let Some(timer) = timers.timers.get_mut(&(pid, timerid)) else {
                return;
            };
            if timer.generation != generation {
                return;
            }
            let Some(now) = posix_timer_clock_now(timer.clock_id, timer.owner_pid, timer.owner_tid)
            else {
                return;
            };
            let missed = if timer.interval.is_zero() {
                1
            } else {
                1 + now
                    .saturating_sub(deadline)
                    .as_nanos()
                    .checked_div(timer.interval.as_nanos())
                    .unwrap_or(0)
            };
            if timer.overrun != i32::MAX {
                timer.overrun = missed.saturating_sub(1).min(i32::MAX as u128) as i32;
            }
            if timer.interval.is_zero() {
                timer.deadline = None;
                (timer.signo, timer.interval, None)
            } else {
                let advance = duration_mul(timer.interval, missed);
                deadline = deadline.checked_add(advance).unwrap_or(TimeValue::MAX);
                timer.deadline = Some(deadline);
                (timer.signo, timer.interval, Some(deadline))
            }
        };

        if let Some(signo) = signo {
            let _ = send_signal_to_process(pid, Some(SignalInfo::new_kernel(signo)));
        }
        if interval.is_zero() || next_deadline.is_none() {
            return;
        }
    }
}

fn valid_posix_timer_clock(clock_id: u32) -> bool {
    matches!(
        clock_id,
        CLOCK_REALTIME
            | CLOCK_MONOTONIC
            | CLOCK_PROCESS_CPUTIME_ID
            | CLOCK_THREAD_CPUTIME_ID
            | CLOCK_BOOTTIME
            | CLOCK_BOOTTIME_ALARM
            | CLOCK_REALTIME_ALARM
            | CLOCK_TAI
    )
}

fn valid_timerfd_clock(clock_id: u32) -> bool {
    matches!(
        clock_id,
        CLOCK_REALTIME
            | CLOCK_MONOTONIC
            | CLOCK_BOOTTIME
            | CLOCK_REALTIME_ALARM
            | CLOCK_BOOTTIME_ALARM
    )
}

fn posix_timer_clock_now(clock_id: u32, pid: Pid, tid: Pid) -> Option<TimeValue> {
    match clock_id {
        CLOCK_REALTIME | CLOCK_REALTIME_ALARM | CLOCK_TAI => Some(wall_time()),
        CLOCK_PROCESS_CPUTIME_ID => process_cpu_time(pid),
        CLOCK_THREAD_CPUTIME_ID => thread_cpu_time(tid),
        _ => Some(monotonic_time()),
    }
}

fn task_cpu_time(task: &axtask::AxTaskRef) -> TimeValue {
    let (utime, stime) = task.as_thread().time.borrow().output();
    time_value_add(utime, stime)
}

fn thread_cpu_time(tid: Pid) -> Option<TimeValue> {
    get_task(tid).ok().map(|task| task_cpu_time(&task))
}

fn process_cpu_time(pid: Pid) -> Option<TimeValue> {
    let proc_data = get_process_data(pid).ok()?;
    let mut total = TimeValue::ZERO;
    for tid in proc_data.proc.threads() {
        if let Some(cpu_time) = thread_cpu_time(tid) {
            total = time_value_add(total, cpu_time);
        }
    }
    Some(total)
}

fn time_value_add(lhs: TimeValue, rhs: TimeValue) -> TimeValue {
    lhs.checked_add(rhs).unwrap_or(TimeValue::MAX)
}

fn duration_mul(duration: TimeValue, count: u128) -> TimeValue {
    if duration.is_zero() || count == 0 {
        return TimeValue::ZERO;
    }
    let nanos = duration.as_nanos().saturating_mul(count);
    if nanos > u64::MAX as u128 {
        TimeValue::MAX
    } else {
        TimeValue::from_nanos(nanos as u64)
    }
}
