use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};
use core::{
    future::poll_fn,
    mem::{MaybeUninit, size_of},
    slice,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};

use axerrno::{AxError, AxResult};
use axhal::{paging::MappingFlags, uspace::UserContext};
use axpoll::PollSet;
use axsync::spin::SpinNoIrq;
use axtask::{current, future::block_on, yield_now};
use linux_raw_sys::general::CLD_TRAPPED;
use memory_addr::{MemoryAddr, VirtAddr};
use starry_process::{Pid, init_proc};
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr, vm_read_slice, vm_write_slice};

use crate::{
    mm::{AddrSpace, IoVec},
    task::{AsThread, ProcessData, Thread, get_process_data, get_task, send_signal_to_process},
};

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
const PTRACE_GETFPREGS: u32 = 14;
const PTRACE_SETFPREGS: u32 = 15;
const PTRACE_ATTACH: u32 = 16;
const PTRACE_DETACH: u32 = 17;
const PTRACE_SYSCALL: u32 = 24;
const PTRACE_SETOPTIONS: u32 = 0x4200;
const PTRACE_GETEVENTMSG: u32 = 0x4201;
const PTRACE_GETSIGINFO: u32 = 0x4202;
const PTRACE_SETSIGINFO: u32 = 0x4203;
const PTRACE_GETREGSET: u32 = 0x4204;
const PTRACE_SETREGSET: u32 = 0x4205;

const NT_PRSTATUS: usize = 1;
const NT_FPREGSET: usize = 2;

const PTRACE_O_TRACESYSGOOD: usize = 1;
const PTRACE_O_TRACEEXEC: usize = 1 << 4;
const PTRACE_O_EXITKILL: usize = 1 << 20;
const PTRACE_SUPPORTED_OPTIONS: usize =
    PTRACE_O_TRACESYSGOOD | PTRACE_O_TRACEEXEC | PTRACE_O_EXITKILL;
const PTRACE_EVENT_EXEC: u32 = 4;

#[cfg(target_arch = "riscv64")]
const SINGLESTEP_BREAK_INSN: u16 = 0x9002;
#[cfg(target_arch = "loongarch64")]
const SINGLESTEP_BREAK_INSN: u32 = 0x002a_0000;

#[cfg(target_arch = "riscv64")]
type ArchUserRegs = RiscvUserRegs;
#[cfg(target_arch = "loongarch64")]
type ArchUserRegs = LoongarchUserRegs;
#[cfg(target_arch = "riscv64")]
type ArchFpRegs = RiscvFpRegs;
#[cfg(target_arch = "loongarch64")]
type ArchFpRegs = LoongarchFpRegs;

#[cfg(target_arch = "riscv64")]
#[repr(C)]
#[derive(Clone, Copy)]
struct RiscvUserRegs {
    pc: usize,
    ra: usize,
    sp: usize,
    gp: usize,
    tp: usize,
    t0: usize,
    t1: usize,
    t2: usize,
    s0: usize,
    s1: usize,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
    a7: usize,
    s2: usize,
    s3: usize,
    s4: usize,
    s5: usize,
    s6: usize,
    s7: usize,
    s8: usize,
    s9: usize,
    s10: usize,
    s11: usize,
    t3: usize,
    t4: usize,
    t5: usize,
    t6: usize,
}

#[cfg(target_arch = "riscv64")]
#[repr(C)]
#[derive(Clone, Copy)]
struct RiscvFpRegs {
    f: [u64; 32],
    fcsr: usize,
}

#[cfg(target_arch = "loongarch64")]
#[repr(C)]
#[derive(Clone, Copy)]
struct LoongarchUserRegs {
    regs: [u64; 32],
    orig_a0: u64,
    csr_era: u64,
    csr_badv: u64,
    reserved: [u64; 10],
}

#[cfg(target_arch = "loongarch64")]
#[repr(C)]
#[derive(Clone, Copy)]
struct LoongarchFpRegs {
    fpr: [u64; 32],
    fcc: u64,
    fcsr: u32,
}

#[cfg(target_arch = "riscv64")]
#[derive(Clone, Copy)]
struct SavedFp(axcpu::FpState);

#[cfg(target_arch = "loongarch64")]
#[derive(Clone, Copy)]
struct SavedFp(axcpu::FpuState);

struct PtraceState {
    tracer: Pid,
    stopped: bool,
    stop_sig: Signo,
    stop_reported: bool,
    stop_event: u32,
    event_msg: usize,
    context: Option<UserContext>,
    fp: Option<SavedFp>,
    siginfo: Option<SignalInfo>,
    pending_stop: Option<Signo>,
    exec_stop_pending: bool,
    resume_signal: Option<Signo>,
    resume_signal_bypass: Option<Signo>,
    detach_pending: bool,
    options: usize,
    single_step: bool,
    saved_step_insn: Option<(usize, usize)>,
    text_dirty: bool,
    resume_event: Arc<PollSet>,
}

impl PtraceState {
    fn new(tracer: Pid) -> Self {
        Self {
            tracer,
            stopped: false,
            stop_sig: Signo::SIGTRAP,
            stop_reported: true,
            stop_event: 0,
            event_msg: 0,
            context: None,
            fp: None,
            siginfo: None,
            pending_stop: None,
            exec_stop_pending: false,
            resume_signal: None,
            resume_signal_bypass: None,
            detach_pending: false,
            options: 0,
            single_step: false,
            saved_step_insn: None,
            text_dirty: false,
            resume_event: Arc::default(),
        }
    }
}

static PTRACE_TABLE: SpinNoIrq<BTreeMap<Pid, PtraceState>> = SpinNoIrq::new(BTreeMap::new());
static PTRACE_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
pub struct PtraceWaitEvent {
    pub pid: Pid,
    pub status: i32,
}

fn make_stop_status(sig: Signo, event: u32) -> i32 {
    ((sig as i32) << 8) | 0x7f | ((event as i32) << 16)
}

fn resolve_tracee_pid(pid: Pid) -> Pid {
    if pid == 1 && get_process_data(1).is_err() {
        init_proc().pid()
    } else {
        pid
    }
}

fn pid_matches(raw_pid: i32, tracee: Pid) -> bool {
    raw_pid == -1 || (raw_pid > 0 && tracee == resolve_tracee_pid(raw_pid as Pid))
}

fn wake_tracer_waiter(tracer: Pid, tracee: Pid) {
    if let Ok(proc_data) = get_process_data(tracer) {
        proc_data.child_exit_event.wake();
    }
    let sigchld = SignalInfo::new_user(Signo::SIGCHLD, CLD_TRAPPED as i32, tracee);
    let _ = send_signal_to_process(tracer, Some(sigchld));
}

fn parse_forward_signal(raw: usize) -> AxResult<Option<Signo>> {
    if raw == 0 {
        return Ok(None);
    }
    let raw = u8::try_from(raw).map_err(|_| AxError::Io)?;
    Signo::from_repr(raw).ok_or(AxError::Io).map(Some)
}

fn check_trace_access(tracee: Pid, tracer: Pid, require_stopped: bool) -> AxResult<()> {
    let table = PTRACE_TABLE.lock();
    let state = table.get(&tracee).ok_or(AxError::NoSuchProcess)?;
    if state.tracer != tracer {
        return Err(AxError::OperationNotPermitted);
    }
    if require_stopped && (!state.stopped || state.context.is_none()) {
        return Err(AxError::NoSuchProcess);
    }
    Ok(())
}

fn allow_attach(curr_euid: u32, target_uid: u32, target_euid: u32) -> bool {
    curr_euid == 0 || curr_euid == target_uid || curr_euid == target_euid
}

pub fn ptrace_tracer_pid(pid: Pid) -> Option<Pid> {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    PTRACE_TABLE.lock().get(&pid).map(|state| state.tracer)
}

pub fn ptrace_stopped_by(pid: Pid, tracer: Pid) -> bool {
    PTRACE_ACTIVE.load(Ordering::Acquire)
        && PTRACE_TABLE
            .lock()
            .get(&pid)
            .is_some_and(|state| state.tracer == tracer && state.stopped && state.context.is_some())
}

pub fn ptrace_note_text_modified(pid: Pid) {
    mark_text_dirty(pid);
}

pub fn ptrace_has_wait_target(tracer: Pid, raw_pid: i32) -> bool {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    PTRACE_TABLE
        .lock()
        .iter()
        .any(|(tracee, state)| state.tracer == tracer && pid_matches(raw_pid, *tracee))
}

pub fn ptrace_take_wait_event(tracer: Pid, raw_pid: i32) -> Option<PtraceWaitEvent> {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let mut table = PTRACE_TABLE.lock();
    let found = table.iter().find_map(|(tracee, state)| {
        (state.tracer == tracer
            && pid_matches(raw_pid, *tracee)
            && state.stopped
            && state.context.is_some()
            && !state.stop_reported)
            .then_some((*tracee, state.stop_sig, state.stop_event))
    })?;
    let (tracee, sig, event) = found;
    table.get_mut(&tracee).unwrap().stop_reported = true;
    Some(PtraceWaitEvent {
        pid: tracee,
        status: make_stop_status(sig, event),
    })
}

#[cfg(target_arch = "riscv64")]
fn save_current_fp() -> SavedFp {
    let mut fp = axcpu::FpState::default();
    fp.save();
    SavedFp(fp)
}

#[cfg(target_arch = "loongarch64")]
fn save_current_fp() -> SavedFp {
    let mut fp = axcpu::FpuState::default();
    fp.save();
    SavedFp(fp)
}

#[cfg(target_arch = "riscv64")]
fn restore_current_fp(mut fp: SavedFp, uctx: &mut UserContext) {
    fp.0.mark_dirty();
    unsafe { riscv::register::sstatus::set_fs(riscv::register::sstatus::FS::Dirty) };
    fp.0.restore();
    uctx.mark_fp_dirty();
}

#[cfg(target_arch = "loongarch64")]
fn restore_current_fp(fp: SavedFp, _uctx: &mut UserContext) {
    fp.0.restore();
}

fn stop_current(thr: &Thread, sig: Signo, event: u32, uctx: &mut UserContext) -> Option<Signo> {
    let pid = thr.proc_data.proc.pid();
    let (tracer, resume_event) = {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&pid) else {
            return None;
        };
        state.stopped = true;
        state.stop_sig = sig;
        state.stop_reported = false;
        state.stop_event = event;
        state.event_msg = 0;
        state.context = Some(*uctx);
        state.fp = Some(save_current_fp());
        state.siginfo = Some(SignalInfo::new_kernel(sig));
        (state.tracer, state.resume_event.clone())
    };
    wake_tracer_waiter(tracer, pid);

    block_on(poll_fn(|cx| {
        let still_stopped = PTRACE_TABLE
            .lock()
            .get(&pid)
            .is_some_and(|state| state.stopped);
        if !still_stopped
            || thr.pending_exit()
            || thr.exec_exit_requested()
            || thr.signal.pending().has(Signo::SIGKILL)
        {
            return Poll::Ready(());
        }
        resume_event.register(cx.waker());
        let _ = current().poll_interrupt(cx);
        if thr.signal.pending().has(Signo::SIGKILL) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }));

    let (context, fp, text_dirty, resume_signal, detach_pending) = {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&pid) else {
            return None;
        };
        if state.stopped {
            return None;
        }
        (
            state.context.take(),
            state.fp.take(),
            core::mem::take(&mut state.text_dirty),
            state.resume_signal.take(),
            state.detach_pending,
        )
    };
    if let Some(context) = context {
        *uctx = context;
    }
    if text_dirty {
        axhal::asm::flush_icache_all();
    }
    if let Some(fp) = fp {
        restore_current_fp(fp, uctx);
    }
    if detach_pending {
        let mut table = PTRACE_TABLE.lock();
        table.remove(&pid);
        PTRACE_ACTIVE.store(!table.is_empty(), Ordering::Release);
    }
    resume_signal
}

/// Stops the current tracee at a pending attach or exec boundary.
pub fn ptrace_wait_if_stopped(thr: &Thread, uctx: &mut UserContext) {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let pid = thr.proc_data.proc.pid();
    let pending = {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&pid) else {
            return;
        };
        if state.exec_stop_pending {
            state.exec_stop_pending = false;
            let event = if state.options & PTRACE_O_TRACEEXEC != 0 {
                PTRACE_EVENT_EXEC
            } else {
                0
            };
            Some((Signo::SIGTRAP, event))
        } else {
            state.pending_stop.take().map(|sig| (sig, 0))
        }
    };
    if let Some((sig, event)) = pending {
        if let Some(resume_sig) = stop_current(thr, sig, event, uctx) {
            if let Some(state) = PTRACE_TABLE.lock().get_mut(&pid) {
                state.resume_signal_bypass = Some(resume_sig);
            }
            let _ = send_signal_to_process(pid, Some(SignalInfo::new_kernel(resume_sig)));
        }
    }
}

pub fn ptrace_is_traced(pid: Pid) -> bool {
    PTRACE_ACTIVE.load(Ordering::Acquire) && PTRACE_TABLE.lock().contains_key(&pid)
}

pub fn ptrace_mark_signal_stop(pid: Pid, sig: Signo) -> bool {
    if sig == Signo::SIGKILL || !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let marked = {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&pid) else {
            return false;
        };
        state.pending_stop = Some(sig);
        true
    };
    if marked && let Ok(task) = get_task(pid) {
        task.interrupt();
    }
    marked
}

/// A fatal SIGKILL must release a ptrace stop before normal signal teardown.
pub fn ptrace_force_kill_wake(pid: Pid) {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let waiter = {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&pid) else {
            return;
        };
        state.stopped = false;
        state.resume_signal = None;
        state.resume_event.clone()
    };
    waiter.wake();
}

pub fn ptrace_intercept_signal(thr: &Thread, sig: Signo, uctx: &mut UserContext) -> bool {
    let pid = thr.proc_data.proc.pid();
    if sig == Signo::SIGKILL || !ptrace_is_traced(pid) {
        return false;
    }
    {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&pid) else {
            return false;
        };
        if state.resume_signal_bypass == Some(sig) {
            state.resume_signal_bypass = None;
            return false;
        }
    }
    match stop_current(thr, sig, 0, uctx) {
        None => true,
        Some(resume_sig) if resume_sig == sig => false,
        Some(resume_sig) => {
            if let Some(state) = PTRACE_TABLE.lock().get_mut(&pid) {
                state.resume_signal_bypass = Some(resume_sig);
            }
            let _ = send_signal_to_process(pid, Some(SignalInfo::new_kernel(resume_sig)));
            true
        }
    }
}

pub fn ptrace_on_exec(pid: Pid) {
    if let Some(state) = PTRACE_TABLE.lock().get_mut(&pid) {
        state.exec_stop_pending = true;
    }
}

pub fn ptrace_on_process_exit(pid: Pid) {
    let (tracee_waiter, traced_by_dead) = {
        let mut table = PTRACE_TABLE.lock();
        let tracee_waiter = table.remove(&pid).map(|state| state.resume_event);
        let traced_by_dead = table
            .iter()
            .filter_map(|(tracee, state)| {
                (state.tracer == pid).then_some((
                    *tracee,
                    state.options,
                    state.resume_event.clone(),
                ))
            })
            .collect::<Vec<_>>();
        for (tracee, ..) in &traced_by_dead {
            table.remove(tracee);
        }
        PTRACE_ACTIVE.store(!table.is_empty(), Ordering::Release);
        (tracee_waiter, traced_by_dead)
    };
    if let Some(waiter) = tracee_waiter {
        waiter.wake();
    }
    for (tracee, options, waiter) in traced_by_dead {
        waiter.wake();
        if options & PTRACE_O_EXITKILL != 0 {
            let _ = send_signal_to_process(tracee, Some(SignalInfo::new_kernel(Signo::SIGKILL)));
        }
    }
}

fn stopped_tracee(pid: Pid, tracer: Pid) -> AxResult<Arc<ProcessData>> {
    let pid = resolve_tracee_pid(pid);
    let tracee = get_process_data(pid)?;
    check_trace_access(pid, tracer, true)?;
    Ok(tracee)
}

fn resume_tracee(pid: Pid, tracer: Pid, sig: Option<Signo>) -> AxResult<()> {
    let pid = resolve_tracee_pid(pid);
    check_trace_access(pid, tracer, true)?;
    let waiter = {
        let mut table = PTRACE_TABLE.lock();
        let state = table.get_mut(&pid).ok_or(AxError::NoSuchProcess)?;
        state.resume_signal = sig;
        state.stopped = false;
        state.stop_event = 0;
        state.resume_event.clone()
    };
    waiter.wake();
    Ok(())
}

pub fn sys_ptrace(request: u32, pid: Pid, addr: usize, data: usize) -> AxResult<isize> {
    let curr = current();
    let curr_thr = curr.as_thread();
    let curr_pid = curr_thr.proc_data.proc.pid();

    match request {
        PTRACE_TRACEME => {
            let parent = curr_thr
                .proc_data
                .proc
                .parent()
                .ok_or(AxError::OperationNotPermitted)?;
            let mut table = PTRACE_TABLE.lock();
            if table.contains_key(&curr_pid) {
                return Err(AxError::OperationNotPermitted);
            }
            table.insert(curr_pid, PtraceState::new(parent.pid()));
            PTRACE_ACTIVE.store(true, Ordering::Release);
            Ok(0)
        }
        PTRACE_ATTACH => {
            let pid = resolve_tracee_pid(pid);
            if pid == 0 || pid == curr_pid {
                return Err(AxError::OperationNotPermitted);
            }
            let target = get_process_data(pid)?;
            if !allow_attach(curr_thr.proc_data.euid(), target.uid(), target.euid()) {
                return Err(AxError::OperationNotPermitted);
            }
            {
                let mut table = PTRACE_TABLE.lock();
                if table.contains_key(&pid) {
                    return Err(AxError::OperationNotPermitted);
                }
                let mut state = PtraceState::new(curr_pid);
                state.pending_stop = Some(Signo::SIGSTOP);
                table.insert(pid, state);
                PTRACE_ACTIVE.store(true, Ordering::Release);
            }
            if let Ok(task) = get_task(pid) {
                task.interrupt();
            }
            Ok(0)
        }
        PTRACE_CONT => {
            let sig = parse_forward_signal(data)?;
            resume_tracee(pid, curr_pid, sig)?;
            yield_now();
            Ok(0)
        }
        PTRACE_SINGLESTEP => {
            let sig = parse_forward_signal(data)?;
            let pid = resolve_tracee_pid(pid);
            stopped_tracee(pid, curr_pid)?;
            setup_software_singlestep(pid)?;
            resume_tracee(pid, curr_pid, sig)?;
            Ok(0)
        }
        PTRACE_SYSCALL => Err(AxError::Unsupported),
        PTRACE_DETACH => {
            let sig = parse_forward_signal(data)?;
            let pid = resolve_tracee_pid(pid);
            check_trace_access(pid, curr_pid, true)?;
            let waiter = {
                let mut table = PTRACE_TABLE.lock();
                let state = table.get_mut(&pid).ok_or(AxError::NoSuchProcess)?;
                state.stopped = false;
                state.resume_signal = sig;
                state.detach_pending = true;
                state.resume_event.clone()
            };
            waiter.wake();
            Ok(0)
        }
        PTRACE_KILL => {
            let pid = resolve_tracee_pid(pid);
            check_trace_access(pid, curr_pid, false)?;
            let waiter = PTRACE_TABLE.lock().get_mut(&pid).map(|state| {
                state.stopped = false;
                state.resume_event.clone()
            });
            if let Some(waiter) = waiter {
                waiter.wake();
            }
            send_signal_to_process(pid, Some(SignalInfo::new_kernel(Signo::SIGKILL)))?;
            Ok(0)
        }
        PTRACE_PEEKTEXT | PTRACE_PEEKDATA => {
            if data == 0 {
                return Err(AxError::InvalidInput);
            }
            let tracee = stopped_tracee(pid, curr_pid)?;
            (data as *mut usize).vm_write(ptrace_read_word(&tracee, addr)?)?;
            Ok(0)
        }
        PTRACE_POKETEXT | PTRACE_POKEDATA => {
            let tracee = stopped_tracee(pid, curr_pid)?;
            ptrace_write_word(&tracee, addr, data)?;
            mark_text_dirty(resolve_tracee_pid(pid));
            Ok(0)
        }
        PTRACE_PEEKUSER => ptrace_peek_user(pid, curr_pid, addr, data),
        PTRACE_POKEUSER => ptrace_poke_user(pid, curr_pid, addr, data),
        PTRACE_GETREGS => ptrace_getregs(pid, curr_pid, data),
        PTRACE_SETREGS => ptrace_setregs(pid, curr_pid, data),
        PTRACE_GETFPREGS => ptrace_getfpregs(pid, curr_pid, data),
        PTRACE_SETFPREGS => ptrace_setfpregs(pid, curr_pid, data),
        PTRACE_SETOPTIONS => {
            if data & !PTRACE_SUPPORTED_OPTIONS != 0 {
                return Err(AxError::InvalidInput);
            }
            let pid = resolve_tracee_pid(pid);
            check_trace_access(pid, curr_pid, true)?;
            PTRACE_TABLE.lock().get_mut(&pid).unwrap().options = data;
            Ok(0)
        }
        PTRACE_GETEVENTMSG => {
            if data == 0 {
                return Err(AxError::InvalidInput);
            }
            let pid = resolve_tracee_pid(pid);
            check_trace_access(pid, curr_pid, true)?;
            let msg = PTRACE_TABLE.lock().get(&pid).unwrap().event_msg;
            (data as *mut usize).vm_write(msg)?;
            Ok(0)
        }
        PTRACE_GETSIGINFO => ptrace_getsiginfo(pid, curr_pid, data),
        PTRACE_SETSIGINFO => ptrace_setsiginfo(pid, curr_pid, data),
        PTRACE_GETREGSET => match addr {
            NT_PRSTATUS => ptrace_getregset(pid, curr_pid, data, false),
            NT_FPREGSET => ptrace_getregset(pid, curr_pid, data, true),
            _ => Err(AxError::Unsupported),
        },
        PTRACE_SETREGSET => match addr {
            NT_PRSTATUS => ptrace_setregset(pid, curr_pid, data, false),
            NT_FPREGSET => ptrace_setregset(pid, curr_pid, data, true),
            _ => Err(AxError::Unsupported),
        },
        _ => Err(AxError::Unsupported),
    }
}

fn stopped_context(pid: Pid, tracer: Pid) -> AxResult<UserContext> {
    let pid = resolve_tracee_pid(pid);
    check_trace_access(pid, tracer, true)?;
    PTRACE_TABLE
        .lock()
        .get(&pid)
        .and_then(|state| state.context)
        .ok_or(AxError::NoSuchProcess)
}

fn replace_stopped_context(pid: Pid, tracer: Pid, context: UserContext) -> AxResult<()> {
    let pid = resolve_tracee_pid(pid);
    check_trace_access(pid, tracer, true)?;
    PTRACE_TABLE.lock().get_mut(&pid).unwrap().context = Some(context);
    Ok(())
}

fn ptrace_getregs(pid: Pid, tracer: Pid, data: usize) -> AxResult<isize> {
    if data == 0 {
        return Err(AxError::InvalidInput);
    }
    let regs = ArchUserRegs::from(&stopped_context(pid, tracer)?);
    let bytes = unsafe {
        slice::from_raw_parts(
            (&regs as *const ArchUserRegs).cast::<u8>(),
            size_of::<ArchUserRegs>(),
        )
    };
    vm_write_slice(data as *mut u8, bytes)?;
    Ok(0)
}

fn read_user_object<T: Copy>(data: usize) -> AxResult<T> {
    if data == 0 {
        return Err(AxError::InvalidInput);
    }
    let mut value = MaybeUninit::<T>::uninit();
    let bytes = unsafe {
        slice::from_raw_parts_mut(value.as_mut_ptr().cast::<MaybeUninit<u8>>(), size_of::<T>())
    };
    vm_read_slice(data as *const u8, bytes)?;
    Ok(unsafe { value.assume_init() })
}

fn ptrace_setregs(pid: Pid, tracer: Pid, data: usize) -> AxResult<isize> {
    let regs = read_user_object::<ArchUserRegs>(data)?;
    let mut context = stopped_context(pid, tracer)?;
    regs.write_to(&mut context)?;
    replace_stopped_context(pid, tracer, context)?;
    Ok(0)
}

fn saved_fp(pid: Pid, tracer: Pid) -> AxResult<SavedFp> {
    let pid = resolve_tracee_pid(pid);
    check_trace_access(pid, tracer, true)?;
    PTRACE_TABLE
        .lock()
        .get(&pid)
        .and_then(|state| state.fp)
        .ok_or(AxError::NoSuchProcess)
}

fn replace_saved_fp(pid: Pid, tracer: Pid, fp: SavedFp) -> AxResult<()> {
    let pid = resolve_tracee_pid(pid);
    check_trace_access(pid, tracer, true)?;
    PTRACE_TABLE.lock().get_mut(&pid).unwrap().fp = Some(fp);
    Ok(())
}

fn ptrace_getfpregs(pid: Pid, tracer: Pid, data: usize) -> AxResult<isize> {
    if data == 0 {
        return Err(AxError::InvalidInput);
    }
    let regs = arch_fp_regs(saved_fp(pid, tracer)?);
    let bytes = unsafe {
        slice::from_raw_parts(
            (&regs as *const ArchFpRegs).cast::<u8>(),
            size_of::<ArchFpRegs>(),
        )
    };
    vm_write_slice(data as *mut u8, bytes)?;
    Ok(0)
}

fn ptrace_setfpregs(pid: Pid, tracer: Pid, data: usize) -> AxResult<isize> {
    let regs = read_user_object::<ArchFpRegs>(data)?;
    let fp = update_saved_fp(saved_fp(pid, tracer)?, regs);
    replace_saved_fp(pid, tracer, fp)?;
    Ok(0)
}

fn ptrace_getregset(pid: Pid, tracer: Pid, data: usize, fp: bool) -> AxResult<isize> {
    if data == 0 {
        return Err(AxError::InvalidInput);
    }
    let mut iov = (data as *const IoVec).vm_read()?;
    if iov.iov_len < 0 {
        return Err(AxError::InvalidInput);
    }
    let regs = if fp {
        let regs = arch_fp_regs(saved_fp(pid, tracer)?);
        unsafe {
            slice::from_raw_parts(
                (&regs as *const ArchFpRegs).cast::<u8>(),
                size_of::<ArchFpRegs>(),
            )
        }
        .to_vec()
    } else {
        let regs = ArchUserRegs::from(&stopped_context(pid, tracer)?);
        unsafe {
            slice::from_raw_parts(
                (&regs as *const ArchUserRegs).cast::<u8>(),
                size_of::<ArchUserRegs>(),
            )
        }
        .to_vec()
    };
    // Native RISC-V GDB determines FLEN by trying exact 33-slot FP regset
    // sizes (first 33 * 4, then 33 * 8).  Linux rejects a short
    // NT_FPREGSET request with EINVAL; silently truncating it makes GDB
    // mis-detect an RV64D target as FLEN=32.
    #[cfg(target_arch = "riscv64")]
    if fp && iov.iov_len as usize != regs.len() {
        return Err(AxError::InvalidInput);
    }
    let len = (iov.iov_len as usize).min(regs.len());
    vm_write_slice(iov.iov_base, &regs[..len])?;
    iov.iov_len = len as isize;
    (data as *mut IoVec).vm_write(iov)?;
    Ok(0)
}

fn ptrace_setregset(pid: Pid, tracer: Pid, data: usize, fp: bool) -> AxResult<isize> {
    if data == 0 {
        return Err(AxError::InvalidInput);
    }
    let iov = (data as *const IoVec).vm_read()?;
    let required = if fp {
        size_of::<ArchFpRegs>()
    } else {
        size_of::<ArchUserRegs>()
    };
    if iov.iov_len < required as isize {
        return Err(AxError::InvalidInput);
    }
    if fp {
        ptrace_setfpregs(pid, tracer, iov.iov_base as usize)
    } else {
        ptrace_setregs(pid, tracer, iov.iov_base as usize)
    }
}

fn ptrace_getsiginfo(pid: Pid, tracer: Pid, data: usize) -> AxResult<isize> {
    if data == 0 {
        return Err(AxError::InvalidInput);
    }
    let pid = resolve_tracee_pid(pid);
    check_trace_access(pid, tracer, true)?;
    let info = PTRACE_TABLE
        .lock()
        .get(&pid)
        .and_then(|state| state.siginfo.clone())
        .ok_or(AxError::NoSuchProcess)?;
    let bytes = unsafe {
        slice::from_raw_parts(
            (&info.0 as *const linux_raw_sys::general::siginfo_t).cast::<u8>(),
            size_of::<linux_raw_sys::general::siginfo_t>(),
        )
    };
    vm_write_slice(data as *mut u8, bytes)?;
    Ok(0)
}

fn ptrace_setsiginfo(pid: Pid, tracer: Pid, data: usize) -> AxResult<isize> {
    let raw = read_user_object::<linux_raw_sys::general::siginfo_t>(data)?;
    let raw_signo = unsafe { raw.__bindgen_anon_1.__bindgen_anon_1.si_signo };
    let signo = Signo::from_repr(raw_signo as u8).ok_or(AxError::InvalidInput)?;
    let pid = resolve_tracee_pid(pid);
    check_trace_access(pid, tracer, true)?;
    let mut table = PTRACE_TABLE.lock();
    let state = table.get_mut(&pid).unwrap();
    state.stop_sig = signo;
    state.siginfo = Some(SignalInfo(raw));
    Ok(0)
}

fn ptrace_peek_user(pid: Pid, tracer: Pid, offset: usize, data: usize) -> AxResult<isize> {
    if data == 0 || !offset.is_multiple_of(size_of::<usize>()) {
        return Err(AxError::Io);
    }
    let regs = ArchUserRegs::from(&stopped_context(pid, tracer)?);
    if offset
        .checked_add(size_of::<usize>())
        .is_none_or(|end| end > size_of::<ArchUserRegs>())
    {
        return Err(AxError::Io);
    }
    let bytes = unsafe {
        slice::from_raw_parts(
            (&regs as *const ArchUserRegs).cast::<u8>(),
            size_of::<ArchUserRegs>(),
        )
    };
    let word = usize::from_ne_bytes(
        bytes[offset..offset + size_of::<usize>()]
            .try_into()
            .unwrap(),
    );
    (data as *mut usize).vm_write(word)?;
    Ok(0)
}

fn ptrace_poke_user(pid: Pid, tracer: Pid, offset: usize, data: usize) -> AxResult<isize> {
    if !offset.is_multiple_of(size_of::<usize>())
        || offset
            .checked_add(size_of::<usize>())
            .is_none_or(|end| end > size_of::<ArchUserRegs>())
    {
        return Err(AxError::Io);
    }
    let mut regs = ArchUserRegs::from(&stopped_context(pid, tracer)?);
    let bytes = unsafe {
        slice::from_raw_parts_mut(
            (&mut regs as *mut ArchUserRegs).cast::<u8>(),
            size_of::<ArchUserRegs>(),
        )
    };
    bytes[offset..offset + size_of::<usize>()].copy_from_slice(&data.to_ne_bytes());
    let mut context = stopped_context(pid, tracer)?;
    regs.write_to(&mut context)?;
    replace_stopped_context(pid, tracer, context)?;
    Ok(0)
}

fn ptrace_populate_remote_range(
    aspace: &mut AddrSpace,
    addr: usize,
    len: usize,
    access: MappingFlags,
) -> AxResult<()> {
    if len == 0 {
        return Ok(());
    }
    let end = addr.checked_add(len).ok_or(AxError::BadAddress)?;
    let start = VirtAddr::from_usize(addr).align_down_4k();
    let end = VirtAddr::from_usize(end).align_up_4k();
    aspace.populate_area(start, end - start, access)
}

fn ptrace_read_word(tracee: &ProcessData, addr: usize) -> AxResult<usize> {
    let aspace = tracee.aspace();
    let mut aspace = aspace.lock();
    ptrace_populate_remote_range(&mut aspace, addr, size_of::<usize>(), MappingFlags::READ)?;
    let mut bytes = [0; size_of::<usize>()];
    aspace.read(VirtAddr::from_usize(addr), &mut bytes)?;
    Ok(usize::from_ne_bytes(bytes))
}

fn ptrace_write_word(tracee: &ProcessData, addr: usize, data: usize) -> AxResult<()> {
    let aspace = tracee.aspace();
    let mut aspace = aspace.lock();
    // Linux permits ptrace to modify a read-only executable VMA. Populating it
    // for read materializes the page; the kernel then writes through its
    // physical mapping without changing user-visible VMA permissions.
    ptrace_populate_remote_range(&mut aspace, addr, size_of::<usize>(), MappingFlags::READ)?;
    aspace.write(VirtAddr::from_usize(addr), &data.to_ne_bytes())
}

fn mark_text_dirty(pid: Pid) {
    if let Some(state) = PTRACE_TABLE.lock().get_mut(&pid) {
        state.text_dirty = true;
    } else {
        axhal::asm::flush_icache_all();
    }
}

#[cfg(target_arch = "riscv64")]
fn read_u16(aspace: &mut AddrSpace, addr: usize) -> AxResult<u16> {
    ptrace_populate_remote_range(aspace, addr, size_of::<u16>(), MappingFlags::READ)?;
    let mut bytes = [0; 2];
    aspace.read(VirtAddr::from_usize(addr), &mut bytes)?;
    Ok(u16::from_ne_bytes(bytes))
}

fn read_u32(aspace: &mut AddrSpace, addr: usize) -> AxResult<u32> {
    ptrace_populate_remote_range(aspace, addr, size_of::<u32>(), MappingFlags::READ)?;
    let mut bytes = [0; 4];
    aspace.read(VirtAddr::from_usize(addr), &mut bytes)?;
    Ok(u32::from_ne_bytes(bytes))
}

#[cfg(target_arch = "riscv64")]
fn write_u16(aspace: &mut AddrSpace, addr: usize, value: u16) -> AxResult<()> {
    ptrace_populate_remote_range(aspace, addr, size_of::<u16>(), MappingFlags::READ)?;
    aspace.write(VirtAddr::from_usize(addr), &value.to_ne_bytes())
}

#[cfg(target_arch = "loongarch64")]
fn write_u32(aspace: &mut AddrSpace, addr: usize, value: u32) -> AxResult<()> {
    ptrace_populate_remote_range(aspace, addr, size_of::<u32>(), MappingFlags::READ)?;
    aspace.write(VirtAddr::from_usize(addr), &value.to_ne_bytes())
}

fn setup_software_singlestep(pid: Pid) -> AxResult<()> {
    let (context, old_saved) = {
        let mut table = PTRACE_TABLE.lock();
        let state = table.get_mut(&pid).ok_or(AxError::NoSuchProcess)?;
        let context = state.context.ok_or(AxError::NoSuchProcess)?;
        (context, state.saved_step_insn.take())
    };
    let tracee = get_process_data(pid)?;
    let aspace = tracee.aspace();
    let mut aspace = aspace.lock();

    if let Some((addr, insn)) = old_saved {
        #[cfg(target_arch = "riscv64")]
        write_u16(&mut aspace, addr, insn as u16)?;
        #[cfg(target_arch = "loongarch64")]
        write_u32(&mut aspace, addr, insn as u32)?;
    }

    let pc = context.ip();
    #[cfg(target_arch = "riscv64")]
    let (next_pc, original) = {
        let half = read_u16(&mut aspace, pc)?;
        let len = riscv_insn_len(half);
        let insn = if len == 2 {
            half as u32
        } else {
            read_u32(&mut aspace, pc)?
        };
        let next = riscv_next_pc(insn, len, pc, &context);
        if next == pc {
            return Err(AxError::InvalidInput);
        }
        (next, read_u16(&mut aspace, next)? as usize)
    };
    #[cfg(target_arch = "loongarch64")]
    let (next_pc, original) = {
        let insn = read_u32(&mut aspace, pc)?;
        let next = loongarch_next_pc(insn, pc, &context);
        (next, read_u32(&mut aspace, next)? as usize)
    };

    #[cfg(target_arch = "riscv64")]
    write_u16(&mut aspace, next_pc, SINGLESTEP_BREAK_INSN)?;
    #[cfg(target_arch = "loongarch64")]
    write_u32(&mut aspace, next_pc, SINGLESTEP_BREAK_INSN)?;
    drop(aspace);

    let mut table = PTRACE_TABLE.lock();
    let state = table.get_mut(&pid).ok_or(AxError::NoSuchProcess)?;
    state.single_step = true;
    state.saved_step_insn = Some((next_pc, original));
    state.text_dirty = true;
    Ok(())
}

/// Restores a temporary software single-step breakpoint when it is hit.
pub fn ptrace_complete_singlestep_breakpoint(pid: Pid, uctx: &mut UserContext) -> bool {
    if !PTRACE_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let saved = {
        let mut table = PTRACE_TABLE.lock();
        let Some(state) = table.get_mut(&pid) else {
            return false;
        };
        if !state.single_step {
            return false;
        }
        state.saved_step_insn.take()
    };
    let Some((addr, insn)) = saved else {
        return false;
    };
    if addr != uctx.ip() {
        if let Some(state) = PTRACE_TABLE.lock().get_mut(&pid) {
            state.saved_step_insn = Some((addr, insn));
        }
        return false;
    }
    let restored = get_process_data(pid).and_then(|tracee| {
        let aspace = tracee.aspace();
        let mut aspace = aspace.lock();
        #[cfg(target_arch = "riscv64")]
        let result = write_u16(&mut aspace, addr, insn as u16);
        #[cfg(target_arch = "loongarch64")]
        let result = write_u32(&mut aspace, addr, insn as u32);
        result
    });
    let mut table = PTRACE_TABLE.lock();
    if let Some(state) = table.get_mut(&pid) {
        if restored.is_ok() {
            state.single_step = false;
            state.text_dirty = true;
        } else {
            state.saved_step_insn = Some((addr, insn));
        }
    }
    restored.is_ok()
}

pub fn sys_process_vm_readv(
    pid: Pid,
    local_iov: *const IoVec,
    local_count: usize,
    remote_iov: *const IoVec,
    remote_count: usize,
    flags: usize,
) -> AxResult<isize> {
    process_vm_copy(
        pid,
        local_iov,
        local_count,
        remote_iov,
        remote_count,
        flags,
        false,
    )
}

pub fn sys_process_vm_writev(
    pid: Pid,
    local_iov: *const IoVec,
    local_count: usize,
    remote_iov: *const IoVec,
    remote_count: usize,
    flags: usize,
) -> AxResult<isize> {
    process_vm_copy(
        pid,
        local_iov,
        local_count,
        remote_iov,
        remote_count,
        flags,
        true,
    )
}

fn process_vm_copy(
    pid: Pid,
    local_iov: *const IoVec,
    local_count: usize,
    remote_iov: *const IoVec,
    remote_count: usize,
    flags: usize,
    write_remote: bool,
) -> AxResult<isize> {
    if flags != 0 {
        return Err(AxError::InvalidInput);
    }
    let curr_pid = current().as_thread().proc_data.proc.pid();
    let pid = resolve_tracee_pid(pid);
    let tracee = if pid == curr_pid {
        get_process_data(pid)?
    } else {
        stopped_tracee(pid, curr_pid)?
    };
    let local = read_iovecs(local_iov, local_count)?;
    let remote = read_iovecs(remote_iov, remote_count)?;
    let mut local_index = 0;
    let mut remote_index = 0;
    let mut local_offset = 0;
    let mut remote_offset = 0;
    let mut copied = 0usize;

    while local_index < local.len() && remote_index < remote.len() {
        skip_empty_iovecs(&local, &mut local_index, &mut local_offset);
        skip_empty_iovecs(&remote, &mut remote_index, &mut remote_offset);
        if local_index == local.len() || remote_index == remote.len() {
            break;
        }
        let len = (local[local_index].iov_len as usize - local_offset)
            .min(remote[remote_index].iov_len as usize - remote_offset);
        let local_addr = local[local_index].iov_base.wrapping_add(local_offset);
        let remote_addr = (remote[remote_index].iov_base as usize)
            .checked_add(remote_offset)
            .ok_or(AxError::BadAddress)?;
        let result = if write_remote {
            let mut bytes = vec![MaybeUninit::<u8>::uninit(); len];
            vm_read_slice(local_addr, &mut bytes)?;
            let bytes = unsafe { slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), len) };
            remote_write(&tracee, remote_addr, bytes)
        } else {
            (|| -> AxResult<()> {
                let bytes = remote_read(&tracee, remote_addr, len)?;
                vm_write_slice(local_addr, &bytes)?;
                Ok(())
            })()
        };
        if let Err(error) = result {
            return if copied == 0 {
                Err(error)
            } else {
                Ok(copied as isize)
            };
        }
        copied = copied.checked_add(len).ok_or(AxError::InvalidInput)?;
        local_offset += len;
        remote_offset += len;
    }
    if write_remote && copied != 0 {
        mark_text_dirty(pid);
    }
    Ok(copied as isize)
}

fn read_iovecs(ptr: *const IoVec, count: usize) -> AxResult<Vec<IoVec>> {
    if count > 1024 {
        return Err(AxError::InvalidInput);
    }
    let mut result = Vec::with_capacity(count);
    let mut total = 0usize;
    for index in 0..count {
        let iov = ptr.wrapping_add(index).vm_read()?;
        if iov.iov_len < 0 {
            return Err(AxError::InvalidInput);
        }
        total = total
            .checked_add(iov.iov_len as usize)
            .filter(|total| *total <= isize::MAX as usize)
            .ok_or(AxError::InvalidInput)?;
        result.push(iov);
    }
    Ok(result)
}

fn skip_empty_iovecs(iovecs: &[IoVec], index: &mut usize, offset: &mut usize) {
    while *index < iovecs.len() && *offset >= iovecs[*index].iov_len as usize {
        *index += 1;
        *offset = 0;
    }
}

fn remote_read(tracee: &ProcessData, addr: usize, len: usize) -> AxResult<Vec<u8>> {
    let aspace = tracee.aspace();
    let mut aspace = aspace.lock();
    ptrace_populate_remote_range(&mut aspace, addr, len, MappingFlags::READ)?;
    let mut bytes = vec![0; len];
    aspace.read(VirtAddr::from_usize(addr), &mut bytes)?;
    Ok(bytes)
}

fn remote_write(tracee: &ProcessData, addr: usize, bytes: &[u8]) -> AxResult<()> {
    let aspace = tracee.aspace();
    let mut aspace = aspace.lock();
    ptrace_populate_remote_range(&mut aspace, addr, bytes.len(), MappingFlags::WRITE)?;
    aspace.write(VirtAddr::from_usize(addr), bytes)
}

#[cfg(target_arch = "riscv64")]
fn riscv_insn_len(first_half: u16) -> usize {
    if first_half & 3 == 3 { 4 } else { 2 }
}

#[cfg(target_arch = "riscv64")]
fn riscv_next_pc(insn: u32, len: usize, pc: usize, uctx: &UserContext) -> usize {
    if len == 2 {
        return riscv_compressed_next_pc(insn as u16, pc, uctx);
    }
    match insn & 0x7f {
        0x63 => {
            let lhs = riscv_reg(uctx, ((insn >> 15) & 0x1f) as usize);
            let rhs = riscv_reg(uctx, ((insn >> 20) & 0x1f) as usize);
            let take = match (insn >> 12) & 7 {
                0 => lhs == rhs,
                1 => lhs != rhs,
                4 => (lhs as isize) < (rhs as isize),
                5 => (lhs as isize) >= (rhs as isize),
                6 => lhs < rhs,
                7 => lhs >= rhs,
                _ => false,
            };
            if take {
                add_signed(
                    pc,
                    sign_extend(
                        (((insn >> 31) & 1) << 12)
                            | (((insn >> 7) & 1) << 11)
                            | (((insn >> 25) & 0x3f) << 5)
                            | (((insn >> 8) & 0xf) << 1),
                        13,
                    ),
                )
            } else {
                pc + 4
            }
        }
        0x67 => {
            add_signed(
                riscv_reg(uctx, ((insn >> 15) & 0x1f) as usize),
                sign_extend((insn >> 20) & 0xfff, 12),
            ) & !1
        }
        0x6f => add_signed(
            pc,
            sign_extend(
                (((insn >> 31) & 1) << 20)
                    | (((insn >> 12) & 0xff) << 12)
                    | (((insn >> 20) & 1) << 11)
                    | (((insn >> 21) & 0x3ff) << 1),
                21,
            ),
        ),
        _ => pc + 4,
    }
}

#[cfg(target_arch = "riscv64")]
fn riscv_compressed_next_pc(insn: u16, pc: usize, uctx: &UserContext) -> usize {
    let quadrant = insn & 3;
    let funct3 = (insn >> 13) & 7;
    if quadrant == 1 {
        match funct3 {
            1 | 5 => return add_signed(pc, sign_extend(riscv_cj_imm(insn) as u32, 12)),
            6 | 7 => {
                let value = riscv_reg(uctx, 8 + ((insn >> 7) & 7) as usize);
                let take = if funct3 == 6 { value == 0 } else { value != 0 };
                return if take {
                    add_signed(pc, sign_extend(riscv_cb_imm(insn) as u32, 9))
                } else {
                    pc + 2
                };
            }
            _ => {}
        }
    }
    if quadrant == 2 && funct3 == 4 {
        let rs1 = ((insn >> 7) & 0x1f) as usize;
        let rs2 = ((insn >> 2) & 0x1f) as usize;
        if rs1 != 0 && rs2 == 0 {
            return riscv_reg(uctx, rs1);
        }
    }
    pc + 2
}

#[cfg(target_arch = "riscv64")]
fn riscv_cj_imm(insn: u16) -> u16 {
    (((insn >> 12) & 1) << 11)
        | (((insn >> 11) & 1) << 4)
        | (((insn >> 9) & 3) << 8)
        | (((insn >> 8) & 1) << 10)
        | (((insn >> 7) & 1) << 6)
        | (((insn >> 6) & 1) << 7)
        | (((insn >> 3) & 7) << 1)
        | (((insn >> 2) & 1) << 5)
}

#[cfg(target_arch = "riscv64")]
fn riscv_cb_imm(insn: u16) -> u16 {
    (((insn >> 12) & 1) << 8)
        | (((insn >> 10) & 3) << 3)
        | (((insn >> 5) & 3) << 6)
        | (((insn >> 3) & 3) << 1)
        | (((insn >> 2) & 1) << 5)
}

fn sign_extend(value: u32, bits: u32) -> isize {
    let shift = usize::BITS - bits;
    ((value as usize) << shift) as isize >> shift
}

fn add_signed(base: usize, offset: isize) -> usize {
    if offset >= 0 {
        base.wrapping_add(offset as usize)
    } else {
        base.wrapping_sub((-offset) as usize)
    }
}

#[cfg(target_arch = "riscv64")]
fn riscv_reg(uctx: &UserContext, index: usize) -> usize {
    let r = &uctx.regs;
    match index {
        0 => 0,
        1 => r.ra,
        2 => r.sp,
        3 => r.gp,
        4 => r.tp,
        5 => r.t0,
        6 => r.t1,
        7 => r.t2,
        8 => r.s0,
        9 => r.s1,
        10 => r.a0,
        11 => r.a1,
        12 => r.a2,
        13 => r.a3,
        14 => r.a4,
        15 => r.a5,
        16 => r.a6,
        17 => r.a7,
        18 => r.s2,
        19 => r.s3,
        20 => r.s4,
        21 => r.s5,
        22 => r.s6,
        23 => r.s7,
        24 => r.s8,
        25 => r.s9,
        26 => r.s10,
        27 => r.s11,
        28 => r.t3,
        29 => r.t4,
        30 => r.t5,
        31 => r.t6,
        _ => 0,
    }
}

#[cfg(target_arch = "loongarch64")]
fn loongarch_next_pc(insn: u32, pc: usize, uctx: &UserContext) -> usize {
    match insn >> 26 {
        0x10 | 0x11 => {
            let value = loongarch_reg(uctx, ((insn >> 5) & 0x1f) as usize);
            let imm = ((insn & 0x1f) << 16) | ((insn >> 10) & 0xffff);
            let take = (value != 0) == (insn >> 26 == 0x11);
            if take {
                add_signed(pc, sign_extend(imm, 21) << 2)
            } else {
                pc + 4
            }
        }
        0x13 => add_signed(
            loongarch_reg(uctx, ((insn >> 5) & 0x1f) as usize),
            sign_extend((insn >> 10) & 0xffff, 16) << 2,
        ),
        0x14 | 0x15 => add_signed(
            pc,
            sign_extend(((insn & 0x3ff) << 16) | ((insn >> 10) & 0xffff), 26) << 2,
        ),
        0x16..=0x1b => {
            let lhs = loongarch_reg(uctx, ((insn >> 5) & 0x1f) as usize);
            let rhs = loongarch_reg(uctx, (insn & 0x1f) as usize);
            let take = match insn >> 26 {
                0x16 => lhs == rhs,
                0x17 => lhs != rhs,
                0x18 => (lhs as isize) < (rhs as isize),
                0x19 => (lhs as isize) >= (rhs as isize),
                0x1a => lhs < rhs,
                0x1b => lhs >= rhs,
                _ => false,
            };
            if take {
                add_signed(pc, sign_extend((insn >> 10) & 0xffff, 16) << 2)
            } else {
                pc + 4
            }
        }
        _ => pc + 4,
    }
}

#[cfg(target_arch = "loongarch64")]
fn loongarch_reg(uctx: &UserContext, index: usize) -> usize {
    let r = &uctx.regs;
    match index {
        0 => 0,
        1 => r.ra,
        2 => r.tp,
        3 => r.sp,
        4 => r.a0,
        5 => r.a1,
        6 => r.a2,
        7 => r.a3,
        8 => r.a4,
        9 => r.a5,
        10 => r.a6,
        11 => r.a7,
        12 => r.t0,
        13 => r.t1,
        14 => r.t2,
        15 => r.t3,
        16 => r.t4,
        17 => r.t5,
        18 => r.t6,
        19 => r.t7,
        20 => r.t8,
        21 => r.u0,
        22 => r.fp,
        23 => r.s0,
        24 => r.s1,
        25 => r.s2,
        26 => r.s3,
        27 => r.s4,
        28 => r.s5,
        29 => r.s6,
        30 => r.s7,
        31 => r.s8,
        _ => 0,
    }
}

#[cfg(target_arch = "riscv64")]
impl From<&UserContext> for RiscvUserRegs {
    fn from(uctx: &UserContext) -> Self {
        let r = &uctx.regs;
        Self {
            pc: uctx.sepc,
            ra: r.ra,
            sp: r.sp,
            gp: r.gp,
            tp: r.tp,
            t0: r.t0,
            t1: r.t1,
            t2: r.t2,
            s0: r.s0,
            s1: r.s1,
            a0: r.a0,
            a1: r.a1,
            a2: r.a2,
            a3: r.a3,
            a4: r.a4,
            a5: r.a5,
            a6: r.a6,
            a7: r.a7,
            s2: r.s2,
            s3: r.s3,
            s4: r.s4,
            s5: r.s5,
            s6: r.s6,
            s7: r.s7,
            s8: r.s8,
            s9: r.s9,
            s10: r.s10,
            s11: r.s11,
            t3: r.t3,
            t4: r.t4,
            t5: r.t5,
            t6: r.t6,
        }
    }
}

#[cfg(target_arch = "riscv64")]
impl RiscvUserRegs {
    fn write_to(&self, uctx: &mut UserContext) -> AxResult<()> {
        uctx.sepc = self.pc;
        let r = &mut uctx.regs;
        r.ra = self.ra;
        r.sp = self.sp;
        r.gp = self.gp;
        r.tp = self.tp;
        r.t0 = self.t0;
        r.t1 = self.t1;
        r.t2 = self.t2;
        r.s0 = self.s0;
        r.s1 = self.s1;
        r.a0 = self.a0;
        r.a1 = self.a1;
        r.a2 = self.a2;
        r.a3 = self.a3;
        r.a4 = self.a4;
        r.a5 = self.a5;
        r.a6 = self.a6;
        r.a7 = self.a7;
        r.s2 = self.s2;
        r.s3 = self.s3;
        r.s4 = self.s4;
        r.s5 = self.s5;
        r.s6 = self.s6;
        r.s7 = self.s7;
        r.s8 = self.s8;
        r.s9 = self.s9;
        r.s10 = self.s10;
        r.s11 = self.s11;
        r.t3 = self.t3;
        r.t4 = self.t4;
        r.t5 = self.t5;
        r.t6 = self.t6;
        Ok(())
    }
}

#[cfg(target_arch = "loongarch64")]
impl From<&UserContext> for LoongarchUserRegs {
    fn from(uctx: &UserContext) -> Self {
        let r = &uctx.regs;
        Self {
            regs: [
                0,
                r.ra as u64,
                r.tp as u64,
                r.sp as u64,
                r.a0 as u64,
                r.a1 as u64,
                r.a2 as u64,
                r.a3 as u64,
                r.a4 as u64,
                r.a5 as u64,
                r.a6 as u64,
                r.a7 as u64,
                r.t0 as u64,
                r.t1 as u64,
                r.t2 as u64,
                r.t3 as u64,
                r.t4 as u64,
                r.t5 as u64,
                r.t6 as u64,
                r.t7 as u64,
                r.t8 as u64,
                r.u0 as u64,
                r.fp as u64,
                r.s0 as u64,
                r.s1 as u64,
                r.s2 as u64,
                r.s3 as u64,
                r.s4 as u64,
                r.s5 as u64,
                r.s6 as u64,
                r.s7 as u64,
                r.s8 as u64,
            ],
            orig_a0: r.a0 as u64,
            csr_era: uctx.era as u64,
            csr_badv: 0,
            reserved: [0; 10],
        }
    }
}

#[cfg(target_arch = "loongarch64")]
impl LoongarchUserRegs {
    fn write_to(&self, uctx: &mut UserContext) -> AxResult<()> {
        let r = &mut uctx.regs;
        r.zero = 0;
        r.ra = self.regs[1] as usize;
        r.tp = self.regs[2] as usize;
        r.sp = self.regs[3] as usize;
        r.a0 = self.regs[4] as usize;
        r.a1 = self.regs[5] as usize;
        r.a2 = self.regs[6] as usize;
        r.a3 = self.regs[7] as usize;
        r.a4 = self.regs[8] as usize;
        r.a5 = self.regs[9] as usize;
        r.a6 = self.regs[10] as usize;
        r.a7 = self.regs[11] as usize;
        r.t0 = self.regs[12] as usize;
        r.t1 = self.regs[13] as usize;
        r.t2 = self.regs[14] as usize;
        r.t3 = self.regs[15] as usize;
        r.t4 = self.regs[16] as usize;
        r.t5 = self.regs[17] as usize;
        r.t6 = self.regs[18] as usize;
        r.t7 = self.regs[19] as usize;
        r.t8 = self.regs[20] as usize;
        r.u0 = self.regs[21] as usize;
        r.fp = self.regs[22] as usize;
        r.s0 = self.regs[23] as usize;
        r.s1 = self.regs[24] as usize;
        r.s2 = self.regs[25] as usize;
        r.s3 = self.regs[26] as usize;
        r.s4 = self.regs[27] as usize;
        r.s5 = self.regs[28] as usize;
        r.s6 = self.regs[29] as usize;
        r.s7 = self.regs[30] as usize;
        r.s8 = self.regs[31] as usize;
        uctx.era = self.csr_era as usize;
        Ok(())
    }
}

#[cfg(target_arch = "riscv64")]
fn arch_fp_regs(saved: SavedFp) -> RiscvFpRegs {
    RiscvFpRegs {
        f: saved.0.fp,
        fcsr: saved.0.fcsr,
    }
}

#[cfg(target_arch = "riscv64")]
fn update_saved_fp(mut saved: SavedFp, regs: RiscvFpRegs) -> SavedFp {
    saved.0.fp = regs.f;
    saved.0.fcsr = regs.fcsr;
    saved
}

#[cfg(target_arch = "loongarch64")]
fn arch_fp_regs(saved: SavedFp) -> LoongarchFpRegs {
    let mut fpr = [0; 32];
    for (dst, src) in fpr.iter_mut().zip(saved.0.regs) {
        *dst = src as u64;
    }
    LoongarchFpRegs {
        fpr,
        fcc: pack_fcc(saved.0.fcc),
        fcsr: saved.0.fcsr,
    }
}

#[cfg(target_arch = "loongarch64")]
fn update_saved_fp(mut saved: SavedFp, regs: LoongarchFpRegs) -> SavedFp {
    for (dst, src) in saved.0.regs.iter_mut().zip(regs.fpr) {
        *dst = (*dst & !u64::MAX as u128) | src as u128;
    }
    saved.0.fcc = unpack_fcc(regs.fcc);
    saved.0.fcsr = regs.fcsr;
    saved
}

#[cfg(target_arch = "loongarch64")]
fn pack_fcc(fcc: [u8; 8]) -> u64 {
    fcc.into_iter().enumerate().fold(0, |value, (index, byte)| {
        value | ((byte as u64) << (index * 8))
    })
}

#[cfg(target_arch = "loongarch64")]
fn unpack_fcc(value: u64) -> [u8; 8] {
    let mut fcc = [0; 8];
    for (index, byte) in fcc.iter_mut().enumerate() {
        *byte = (value >> (index * 8)) as u8;
    }
    fcc
}
