use alloc::{format, string::String, vec::Vec};
use core::{
    arch::asm,
    fmt::Write,
    mem::{align_of, size_of},
    sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
    time::Duration,
};

use axcpu::{
    TrapFrame,
    trap::{PROFILE_SAMPLE, register_trap_handler},
};
use axsync::{ProfileEvent, spin::SpinNoIrq};

const STACK_DEPTH: usize = 20;
const CPU_TABLE_SIZE: usize = 4096;
const WAIT_TABLE_SIZE: usize = 8192;
const PENDING_SIZE: usize = 512;
const SAMPLE_TICK_DIVISOR: u32 = 10;
const EXT4_SAMPLE_RATE: u32 = 16;
const PAGE_CACHE_SAMPLE_RATE: u32 = 32;
const OFFCPU_SAMPLE_RATE: u32 = 8;
const EVENT_COUNT: usize = 11;
const USER_SPACE_FRAME: usize = usize::MAX;
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);
const LONG_WAIT_NS: u64 = 30_000_000_000;
const TASK_NAME_LEN: usize = 16;

static ENABLED: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU8 = AtomicU8::new(0);
static PHASE_STARTED_NS: AtomicU64 = AtomicU64::new(0);
static PHASE_TOTAL_NS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static SKIPPED_CPU: AtomicU64 = AtomicU64::new(0);
static WATCHDOG_TASK_ID: AtomicU64 = AtomicU64::new(0);
static CPU_TICKS: [AtomicU32; axconfig::plat::CPU_NUM] =
    [const { AtomicU32::new(0) }; axconfig::plat::CPU_NUM];
static EVENT_TICKS: [[AtomicU32; EVENT_COUNT]; axconfig::plat::CPU_NUM] =
    [const { [const { AtomicU32::new(0) }; EVENT_COUNT] }; axconfig::plat::CPU_NUM];
static PROFILES: [SpinNoIrq<ProfileState>; axconfig::plat::CPU_NUM] =
    [const { SpinNoIrq::new(ProfileState::new()) }; axconfig::plat::CPU_NUM];

#[derive(Clone, Copy, Eq, PartialEq)]
struct StackTrace {
    task_name: TaskName,
    len: u8,
    frames: [usize; STACK_DEPTH],
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct TaskName {
    len: u8,
    bytes: [u8; TASK_NAME_LEN],
}

impl TaskName {
    const EMPTY: Self = Self {
        len: 0,
        bytes: [0; TASK_NAME_LEN],
    };

    fn current() -> Self {
        let mut bytes = [0; TASK_NAME_LEN];
        let len = axtask::current_may_uninit()
            .map(|task| task.copy_name(&mut bytes))
            .unwrap_or(0);
        Self {
            len: len as u8,
            bytes,
        }
    }
}

impl StackTrace {
    const EMPTY: Self = Self {
        task_name: TaskName::EMPTY,
        len: 0,
        frames: [0; STACK_DEPTH],
    };

    fn current() -> Self {
        Self {
            task_name: TaskName::current(),
            ..Self::EMPTY
        }
    }

    fn push(&mut self, ip: usize) {
        if ip != 0 && (self.len as usize) < STACK_DEPTH {
            self.frames[self.len as usize] = ip;
            self.len += 1;
        }
    }

    fn hash(&self, phase: u8, event: u8) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for value in [phase as u64, event as u64, self.len as u64] {
            hash ^= value;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        for &byte in &self.task_name.bytes[..self.task_name.len as usize] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        for &ip in &self.frames[..self.len as usize] {
            hash ^= ip as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        hash.max(1)
    }
}

#[derive(Clone, Copy)]
struct Aggregate {
    hash: u64,
    phase: u8,
    event: u8,
    stack: StackTrace,
    total: u64,
    count: u64,
    max: u64,
}

impl Aggregate {
    const EMPTY: Self = Self {
        hash: 0,
        phase: 0,
        event: 0,
        stack: StackTrace::EMPTY,
        total: 0,
        count: 0,
        max: 0,
    };
}

#[derive(Clone, Copy)]
struct Pending {
    active: bool,
    reported: bool,
    generation: u32,
    phase: u8,
    event: u8,
    scale: u32,
    task_id: u64,
    object: usize,
    started_ns: u64,
    stack: StackTrace,
}

impl Pending {
    const EMPTY: Self = Self {
        active: false,
        reported: false,
        generation: 0,
        phase: 0,
        event: 0,
        scale: 1,
        task_id: 0,
        object: 0,
        started_ns: 0,
        stack: StackTrace::EMPTY,
    };
}

struct ProfileState {
    cpu: [Aggregate; CPU_TABLE_SIZE],
    wait: [Aggregate; WAIT_TABLE_SIZE],
    pending: [Pending; PENDING_SIZE],
    next_pending: usize,
    dropped_cpu: u64,
    dropped_wait: u64,
    dropped_pending: u64,
}

impl ProfileState {
    const fn new() -> Self {
        Self {
            cpu: [Aggregate::EMPTY; CPU_TABLE_SIZE],
            wait: [Aggregate::EMPTY; WAIT_TABLE_SIZE],
            pending: [Pending::EMPTY; PENDING_SIZE],
            next_pending: 0,
            dropped_cpu: 0,
            dropped_wait: 0,
            dropped_pending: 0,
        }
    }

    fn clear(&mut self) {
        self.cpu.fill(Aggregate::EMPTY);
        self.wait.fill(Aggregate::EMPTY);
        self.pending.fill(Pending::EMPTY);
        self.next_pending = 0;
        self.dropped_cpu = 0;
        self.dropped_wait = 0;
        self.dropped_pending = 0;
    }

    fn add_cpu(&mut self, phase: u8, stack: StackTrace) {
        if !add_aggregate(&mut self.cpu, phase, 0, stack, 1, 1) {
            self.dropped_cpu += 1;
        }
    }

    fn add_wait(&mut self, phase: u8, event: u8, stack: StackTrace, duration_ns: u64, scale: u32) {
        if !add_aggregate(
            &mut self.wait,
            phase,
            event,
            stack,
            duration_ns.saturating_mul(scale as u64),
            duration_ns,
        ) {
            self.dropped_wait += 1;
        }
    }
}

fn add_aggregate<const N: usize>(
    table: &mut [Aggregate; N],
    phase: u8,
    event: u8,
    stack: StackTrace,
    total_value: u64,
    max_value: u64,
) -> bool {
    let hash = stack.hash(phase, event);
    let mut index = hash as usize & (N - 1);
    for _ in 0..N {
        let entry = &mut table[index];
        if entry.hash == 0 {
            *entry = Aggregate {
                hash,
                phase,
                event,
                stack,
                total: total_value,
                count: 1,
                max: max_value,
            };
            return true;
        }
        if entry.hash == hash
            && entry.phase == phase
            && entry.event == event
            && entry.stack == stack
        {
            entry.total = entry.total.saturating_add(total_value);
            entry.count = entry.count.saturating_add(1);
            entry.max = entry.max.max(max_value);
            return true;
        }
        index = (index + 1) & (N - 1);
    }
    false
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FrameRecord {
    previous_fp: usize,
    return_address: usize,
}

unsafe extern "C" {
    fn _stext();
    fn _etext();
}

#[inline]
fn is_kernel_text(ip: usize) -> bool {
    ip >= _stext as usize && ip < _etext as usize
}

fn unwind_kernel_stack(stack: &mut StackTrace, mut fp: usize) {
    let Some(range) = axtask::current_may_uninit().and_then(|task| task.kernel_stack_range())
    else {
        return;
    };

    while stack.len as usize != STACK_DEPTH {
        if !fp.is_multiple_of(align_of::<FrameRecord>())
            || fp < range.start.saturating_add(size_of::<FrameRecord>())
            || fp > range.end
        {
            break;
        }
        // RISC-V stores the previous frame pointer and return address in the
        // record immediately below the current frame pointer.
        let record = unsafe { (fp as *const FrameRecord).sub(1).read() };
        let return_address = record.return_address.wrapping_sub(1);
        if !is_kernel_text(return_address) {
            break;
        }
        stack.push(return_address);
        if record.previous_fp <= fp || record.previous_fp > range.end {
            break;
        }
        fp = record.previous_fp;
    }
}

fn capture_current_stack() -> StackTrace {
    let fp: usize;
    let ra: usize;
    unsafe {
        asm!("mv {fp}, s0", "mv {ra}, ra", fp = out(reg) fp, ra = out(reg) ra);
    }
    let mut stack = StackTrace::current();
    stack.push(ra.wrapping_sub(1));
    unwind_kernel_stack(&mut stack, fp);
    stack
}

fn capture_trap_stack(tf: &TrapFrame) -> StackTrace {
    let mut stack = StackTrace::current();
    if tf.sstatus.bits() & (1 << 8) == 0 {
        stack.push(USER_SPACE_FRAME);
        return stack;
    }
    stack.push(tf.sepc);
    unwind_kernel_stack(&mut stack, tf.regs.s0);
    stack
}

fn profile_begin(event: ProfileEvent, object: usize) -> u64 {
    if !ENABLED.load(Ordering::Relaxed) {
        return 0;
    }
    let phase = PHASE.load(Ordering::Relaxed);
    if phase == 0 {
        return 0;
    }

    let cpu = axhal::percpu::this_cpu_id();
    let scale = match event {
        ProfileEvent::Ext4 => EXT4_SAMPLE_RATE,
        ProfileEvent::PageCache => PAGE_CACHE_SAMPLE_RATE,
        ProfileEvent::OffCpu => OFFCPU_SAMPLE_RATE,
        _ => 1,
    };
    if EVENT_TICKS[cpu][event as usize].fetch_add(1, Ordering::Relaxed) % scale != 0 {
        return 0;
    }
    let stack = capture_current_stack();
    let started_ns = axhal::time::monotonic_time_nanos();
    let mut state = PROFILES[cpu].lock();
    for offset in 0..PENDING_SIZE {
        let index = (state.next_pending + offset) % PENDING_SIZE;
        if !state.pending[index].active {
            let generation = state.pending[index].generation.wrapping_add(1).max(1);
            state.pending[index] = Pending {
                active: true,
                reported: false,
                generation,
                phase,
                event: event as u8,
                scale,
                task_id: axtask::current().id().as_u64(),
                object,
                started_ns,
                stack,
            };
            state.next_pending = (index + 1) % PENDING_SIZE;
            return ((generation as u64) << 32) | ((cpu as u64) << 16) | (index as u64 + 1);
        }
    }
    state.dropped_pending += 1;
    0
}

fn profile_end(token: u64) {
    let raw_index = token as u16;
    if raw_index == 0 {
        return;
    }
    let index = raw_index as usize - 1;
    if index >= PENDING_SIZE {
        return;
    }
    let cpu = ((token >> 16) & 0xffff) as usize;
    if cpu >= PROFILES.len() {
        return;
    }
    let generation = (token >> 32) as u32;
    let finished_ns = axhal::time::monotonic_time_nanos();
    let mut state = PROFILES[cpu].lock();
    let pending = state.pending[index];
    if !pending.active || pending.generation != generation {
        return;
    }
    state.pending[index].active = false;
    state.add_wait(
        pending.phase,
        pending.event,
        pending.stack,
        finished_ns.saturating_sub(pending.started_ns),
        pending.scale,
    );
}

#[register_trap_handler(PROFILE_SAMPLE)]
fn profile_timer_sample(tf: &TrapFrame) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let phase = PHASE.load(Ordering::Relaxed);
    if phase == 0 {
        return;
    }
    let cpu = axhal::percpu::this_cpu_id();
    if CPU_TICKS[cpu].fetch_add(1, Ordering::Relaxed) % SAMPLE_TICK_DIVISOR != 0 {
        return;
    }

    let stack = capture_trap_stack(tf);
    if let Some(mut state) = PROFILES[cpu].try_lock() {
        state.add_cpu(phase, stack);
    } else {
        SKIPPED_CPU.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn init() {
    axsync::register_profile_hooks(profile_begin, profile_end);
    axtask::future::register_block_profile_hooks(profile_block_begin, profile_end);
    let watchdog = axtask::spawn_with_name(
        || loop {
            axtask::sleep(WATCHDOG_INTERVAL);
            report_long_waits();
        },
        "profile-watchdog".into(),
    );
    WATCHDOG_TASK_ID.store(watchdog.id().as_u64(), Ordering::Release);
}

fn profile_block_begin() -> u64 {
    if axtask::current().id().as_u64() == WATCHDOG_TASK_ID.load(Ordering::Acquire) {
        return 0;
    }
    profile_begin(ProfileEvent::OffCpu, 0)
}

#[derive(Clone, Copy)]
struct LongWait {
    phase: u8,
    event: u8,
    task_id: u64,
    object: usize,
    elapsed_ns: u64,
    stack: StackTrace,
}

fn report_long_waits() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }

    let now = axhal::time::monotonic_time_nanos();
    // Reserve before taking a profile shard lock. Allocation can schedule and
    // must never happen while a non-preemptible profiling lock is held.
    let mut waits = Vec::with_capacity(PROFILES.len() * PENDING_SIZE);
    for profile in &PROFILES {
        let mut state = profile.lock();
        for pending in state.pending.iter_mut().filter(|pending| pending.active) {
            let elapsed_ns = now.saturating_sub(pending.started_ns);
            if !pending.reported && elapsed_ns >= LONG_WAIT_NS {
                pending.reported = true;
                waits.push(LongWait {
                    phase: pending.phase,
                    event: pending.event,
                    task_id: pending.task_id,
                    object: pending.object,
                    elapsed_ns,
                    stack: pending.stack,
                });
            }
        }
    }

    if waits.is_empty() {
        return;
    }

    let mut output = String::from("#### MOSS LONG WAITS START ####\n");
    for wait in waits {
        let _ = write!(
            output,
            "MOSS_LONG_WAIT phase={} event={} task={} object={:#x} elapsed_ns={} name=",
            wait.phase, wait.event, wait.task_id, wait.object, wait.elapsed_ns,
        );
        write_task_name(&mut output, &wait.stack.task_name);
        output.push_str(" stack=");
        write_stack(&mut output, &wait.stack);
        output.push('\n');
    }
    output.push_str("#### MOSS LONG WAITS END ####\n");
    axhal::console::write_bytes(output.as_bytes());
}

fn switch_phase(next: u8) {
    let now = axhal::time::monotonic_time_nanos();
    let previous = PHASE.swap(next, Ordering::SeqCst);
    let started = PHASE_STARTED_NS.swap(if next == 0 { 0 } else { now }, Ordering::SeqCst);
    if previous != 0 && started != 0 {
        PHASE_TOTAL_NS[previous as usize].fetch_add(now.saturating_sub(started), Ordering::Relaxed);
    }
}

pub fn command(input: &[u8]) {
    let command = core::str::from_utf8(input).unwrap_or("").trim();
    match command {
        "reset" => {
            ENABLED.store(false, Ordering::SeqCst);
            switch_phase(0);
            for profile in &PROFILES {
                profile.lock().clear();
            }
            for ticks in &CPU_TICKS {
                ticks.store(0, Ordering::Relaxed);
            }
            for cpu_ticks in &EVENT_TICKS {
                for ticks in cpu_ticks {
                    ticks.store(0, Ordering::Relaxed);
                }
            }
            SKIPPED_CPU.store(0, Ordering::Relaxed);
            for elapsed in &PHASE_TOTAL_NS {
                elapsed.store(0, Ordering::Relaxed);
            }
        }
        "start" => ENABLED.store(true, Ordering::Release),
        "stop" => {
            ENABLED.store(false, Ordering::Release);
            switch_phase(0);
        }
        "phase=setup" => {
            switch_phase(1);
            ENABLED.store(true, Ordering::Release);
        }
        "phase=prebuild" => {
            switch_phase(2);
            ENABLED.store(true, Ordering::Release);
        }
        "phase=timed" => {
            switch_phase(3);
            ENABLED.store(true, Ordering::Release);
        }
        _ => {}
    }
}

fn write_stack(output: &mut String, stack: &StackTrace) {
    for (index, ip) in stack.frames[..stack.len as usize].iter().enumerate() {
        if index != 0 {
            output.push(';');
        }
        if *ip == USER_SPACE_FRAME {
            output.push_str("user");
        } else {
            let _ = write!(output, "{ip:#x}");
        }
    }
}

fn write_task_name(output: &mut String, task_name: &TaskName) {
    for byte in &task_name.bytes[..task_name.len as usize] {
        let _ = write!(output, "{byte:02x}");
    }
}

pub fn snapshot() -> String {
    let mut dropped_cpu = 0;
    let mut dropped_wait = 0;
    let mut dropped_pending = 0;
    for profile in &PROFILES {
        let state = profile.lock();
        dropped_cpu += state.dropped_cpu;
        dropped_wait += state.dropped_wait;
        dropped_pending += state.dropped_pending;
    }
    let mut output = format!(
        "MOSS_PROFILE_V1 sample_hz={} ext4_sample_rate={} page_cache_sample_rate={} \
         offcpu_sample_rate={} setup_ns={} prebuild_ns={} timed_ns={} dropped_cpu={} \
         dropped_wait={} dropped_pending={} skipped_cpu={}\n",
        axconfig::TICKS_PER_SEC / SAMPLE_TICK_DIVISOR as usize,
        EXT4_SAMPLE_RATE,
        PAGE_CACHE_SAMPLE_RATE,
        OFFCPU_SAMPLE_RATE,
        PHASE_TOTAL_NS[1].load(Ordering::Relaxed),
        PHASE_TOTAL_NS[2].load(Ordering::Relaxed),
        PHASE_TOTAL_NS[3].load(Ordering::Relaxed),
        dropped_cpu,
        dropped_wait,
        dropped_pending,
        SKIPPED_CPU.load(Ordering::Relaxed),
    );
    output.push_str("MOSS_PHASE 1 setup\nMOSS_PHASE 2 prebuild\nMOSS_PHASE 3 timed\n");
    for profile in &PROFILES {
        // Formatting and growing the output string may allocate. Copy the
        // compact records while holding the shard, then release it first.
        let mut cpu_entries = Vec::with_capacity(CPU_TABLE_SIZE);
        let mut wait_entries = Vec::with_capacity(WAIT_TABLE_SIZE);
        {
            let state = profile.lock();
            cpu_entries.extend(state.cpu.iter().filter(|entry| entry.hash != 0).copied());
            wait_entries.extend(state.wait.iter().filter(|entry| entry.hash != 0).copied());
        }
        for entry in &cpu_entries {
            let _ = write!(
                output,
                "MOSS_CPU phase={} samples={} task=",
                entry.phase, entry.total
            );
            write_task_name(&mut output, &entry.stack.task_name);
            output.push_str(" stack=");
            write_stack(&mut output, &entry.stack);
            output.push('\n');
        }
        for entry in &wait_entries {
            let _ = write!(
                output,
                "MOSS_WAIT phase={} event={} count={} total_ns={} max_ns={} task=",
                entry.phase, entry.event, entry.count, entry.total, entry.max
            );
            write_task_name(&mut output, &entry.stack.task_name);
            output.push_str(" stack=");
            write_stack(&mut output, &entry.stack);
            output.push('\n');
        }
    }
    output
}
