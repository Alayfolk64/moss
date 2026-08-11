//! Task APIs for multi-task configuration.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use alloc::{
    string::String,
    sync::{Arc, Weak},
};

use kernel_guard::NoPreemptIrqSave;

pub(crate) use crate::run_queue::{current_run_queue, select_run_queue};

#[doc(cfg(feature = "multitask"))]
pub use crate::{
    task::{CurrentTask, TaskId, TaskInner, TaskState},
    wait_queue::WaitQueue,
};

#[doc(cfg(all(feature = "multitask", feature = "irq")))]
#[cfg(feature = "irq")]
pub use crate::timers::register_timer_callback;

#[doc(cfg(all(feature = "multitask", feature = "task-ext")))]
#[cfg(feature = "task-ext")]
pub use crate::task::{AxTaskExt, TaskExt};

/// The reference type of a task.
pub type AxTaskRef = Arc<AxTask>;

/// The weak reference type of a task.
pub type WeakAxTaskRef = Weak<AxTask>;

/// The wrapper type for [`cpumask::CpuMask`] with SMP configuration.
pub type AxCpuMask = cpumask::CpuMask<{ axconfig::plat::CPU_NUM }>;

static CPU_NUM: AtomicUsize = AtomicUsize::new(1);
static CPU_USER_TASK_NANOS: [AtomicU64; axconfig::plat::CPU_NUM] =
    [const { AtomicU64::new(0) }; axconfig::plat::CPU_NUM];
static CPU_LAST_ACCOUNT_NS: [AtomicU64; axconfig::plat::CPU_NUM] =
    [const { AtomicU64::new(0) }; axconfig::plat::CPU_NUM];

cfg_if::cfg_if! {
    if #[cfg(feature = "sched-rr")] {
        const MAX_TIME_SLICE: usize = 5;
        pub(crate) type AxTask = axsched::RRTask<TaskInner, MAX_TIME_SLICE>;
        pub(crate) type Scheduler = axsched::RRScheduler<TaskInner, MAX_TIME_SLICE>;
    } else if #[cfg(feature = "sched-cfs")] {
        pub(crate) type AxTask = axsched::CFSTask<TaskInner>;
        pub(crate) type Scheduler = axsched::CFScheduler<TaskInner>;
    } else {
        // If no scheduler features are set, use FIFO as the default.
        pub(crate) type AxTask = axsched::FifoTask<TaskInner>;
        pub(crate) type Scheduler = axsched::FifoScheduler<TaskInner>;
    }
}

#[cfg(feature = "preempt")]
struct KernelGuardIfImpl;

#[cfg(feature = "preempt")]
#[crate_interface::impl_interface]
impl kernel_guard::KernelGuardIf for KernelGuardIfImpl {
    fn disable_preempt() {
        if let Some(curr) = current_may_uninit() {
            curr.disable_preempt();
        }
    }

    fn disable_preempt_irq_disabled() {
        // NoPreemptIrqSave has already disabled local IRQs, so this per-CPU
        // read cannot race with task migration.
        if let Some(curr) = unsafe { CurrentTask::try_get_migration_disabled() } {
            curr.disable_preempt();
        }
    }

    fn enable_preempt() {
        // A preemption guard remains active until this hook returns.
        if let Some(curr) = unsafe { CurrentTask::try_get_migration_disabled() } {
            curr.enable_preempt(true);
        }
    }
}

/// Gets the current task, or returns [`None`] if the current task is not
/// initialized.
pub fn current_may_uninit() -> Option<CurrentTask> {
    CurrentTask::try_get()
}

/// Returns whether the current task may safely enter a blocking wait.
///
/// The scheduler bootstrap task and per-CPU idle tasks must keep polling even
/// after the scheduler has been initialized. Only ordinary spawned tasks have
/// a context that can be placed on a wait queue and resumed later.
pub fn current_can_block() -> bool {
    current_may_uninit().is_some_and(|task| {
        crate::block_context::task_context_may_block(task.is_init(), task.is_idle())
    })
}

/// Replace the page-table root saved for the current task and activate it.
///
/// The caller must keep the old address space alive until this function
/// returns.
pub fn replace_current_page_table(root: memory_addr::PhysAddr) {
    let _guard = NoPreemptIrqSave::new();
    let curr = current();
    unsafe {
        (*curr.ctx_mut_ptr()).set_page_table_root(root);
        axhal::asm::write_user_page_table(root);
    }
    axhal::asm::flush_tlb(None);
}

/// Gets the current task.
///
/// # Panics
///
/// Panics if the current task is not initialized.
pub fn current() -> CurrentTask {
    CurrentTask::get()
}

/// Looks up a live scheduler task by its unique ID.
pub fn task_by_id(id: u64) -> Option<AxTaskRef> {
    crate::task::task_by_id(id)
}

/// Reports recovery of a mutex whose recorded owner no longer exists.
pub fn report_recovered_mutex(
    address: usize,
    stale_owner_id: u64,
    caller: &'static core::panic::Location<'static>,
) {
    warn!(
        "recovered orphaned/corrupt mutex: address={:#x}, stale_owner_tid={}, caller={:?}",
        address, stale_owner_id, caller
    );
}

/// Initializes the task scheduler (for the primary CPU).
pub fn init_scheduler() {
    init_scheduler_with_cpu_num(axhal::cpu_num());
}

/// Initializes the task scheduler with cpu_num (for the primary CPU).
pub fn init_scheduler_with_cpu_num(cpu_num: usize) {
    info!("Initialize scheduling...");
    CPU_NUM.store(cpu_num, core::sync::atomic::Ordering::Relaxed);

    crate::run_queue::init();

    info!("  use {} scheduler.", Scheduler::scheduler_name());
}

pub(crate) fn active_cpu_num() -> usize {
    active_cpu_count()
}

/// Returns the number of CPUs currently managed by the scheduler.
pub fn active_cpu_count() -> usize {
    CPU_NUM.load(core::sync::atomic::Ordering::Relaxed)
}

/// Returns timer-accounted CPU time spent executing a userspace-owned task.
/// This includes real syscall work on behalf of that task, but excludes idle
/// and standalone kernel workers. The latter distinction prevents background
/// reclaim from renewing a userspace forward-progress lease by merely scanning.
pub fn cpu_user_task_time_nanos(cpu_id: usize) -> Option<u64> {
    (cpu_id < active_cpu_count())
        .then(|| CPU_USER_TASK_NANOS[cpu_id].load(Ordering::Relaxed))
}

#[cfg(feature = "task-ext")]
fn account_user_task_time() {
    let cpu_id = axhal::percpu::this_cpu_id();
    let now_ns = axhal::time::monotonic_time_nanos();
    let previous_ns = CPU_LAST_ACCOUNT_NS[cpu_id].swap(now_ns, Ordering::Relaxed);
    if previous_ns == 0 {
        return;
    }
    if current_may_uninit().is_some_and(|task| task.task_ext().is_some()) {
        CPU_USER_TASK_NANOS[cpu_id]
            .fetch_add(now_ns.saturating_sub(previous_ns), Ordering::Relaxed);
    }
}

/// Initializes the task scheduler for secondary CPUs.
pub fn init_scheduler_secondary() {
    crate::run_queue::init_secondary();
}

/// Handles periodic timer ticks for the task manager.
///
/// For example, advance scheduler states, checks timed events, etc.
#[cfg(feature = "irq")]
#[doc(cfg(feature = "irq"))]
pub fn on_timer_tick() {
    use kernel_guard::NoOp;
    #[cfg(feature = "task-ext")]
    account_user_task_time();
    crate::timers::check_events();
    // Since irq and preemption are both disabled here,
    // we can get current run queue with the default `kernel_guard::NoOp`.
    current_run_queue::<NoOp>().scheduler_timer_tick();
}

/// Adds the given task to the run queue, returns the task reference.
pub fn spawn_task(task: TaskInner) -> AxTaskRef {
    let task_ref = task.into_arc();
    select_run_queue::<NoPreemptIrqSave>(&task_ref).add_task(task_ref.clone());
    task_ref
}

/// Spawns a new task with the given parameters.
///
/// Returns the task reference.
pub fn spawn_raw<F>(f: F, name: String, stack_size: usize) -> AxTaskRef
where
    F: FnOnce() + Send + 'static,
{
    spawn_task(TaskInner::new(f, name, stack_size))
}

/// Spawns a new task with the given name and the default stack size ([`axconfig::TASK_STACK_SIZE`]).
///
/// Returns the task reference.
pub fn spawn_with_name<F>(f: F, name: String) -> AxTaskRef
where
    F: FnOnce() + Send + 'static,
{
    spawn_raw(f, name, axconfig::TASK_STACK_SIZE)
}

/// Spawns a new task with the default parameters.
///
/// The default task name is an empty string. The default task stack size is
/// [`axconfig::TASK_STACK_SIZE`].
///
/// Returns the task reference.
pub fn spawn<F>(f: F) -> AxTaskRef
where
    F: FnOnce() + Send + 'static,
{
    spawn_with_name(f, String::new())
}

/// Set the priority for current task.
///
/// The range of the priority is dependent on the underlying scheduler. For
/// example, in the [CFS] scheduler, the priority is the nice value, ranging from
/// -20 to 19.
///
/// Returns `true` if the priority is set successfully.
///
/// [CFS]: https://en.wikipedia.org/wiki/Completely_Fair_Scheduler
pub fn set_priority(prio: isize) -> bool {
    current_run_queue::<NoPreemptIrqSave>().set_current_priority(prio)
}

/// Set the affinity for the current task.
/// [`AxCpuMask`] is used to specify the CPU affinity.
/// Returns `true` if the affinity is set successfully.
///
/// TODO: support set the affinity for other tasks.
pub fn set_current_affinity(cpumask: AxCpuMask) -> bool {
    if cpumask.is_empty() {
        false
    } else {
        let curr = current().clone();

        curr.set_cpumask(cpumask);
        // After setting the affinity, we need to check if current cpu matches
        // the affinity. If not, we need to migrate the task to the correct CPU.
        #[cfg(feature = "smp")]
        if !cpumask.get(axhal::percpu::this_cpu_id()) {
            const MIGRATION_TASK_STACK_SIZE: usize = 4096;
            // Spawn a new migration task for migrating.
            let migration_task = TaskInner::new(
                move || crate::run_queue::migrate_entry(curr),
                "migration-task".into(),
                MIGRATION_TASK_STACK_SIZE,
            )
            .into_arc();

            // Migrate the current task to the correct CPU using the migration task.
            current_run_queue::<NoPreemptIrqSave>().migrate_current(migration_task);

            assert!(
                cpumask.get(axhal::percpu::this_cpu_id()),
                "Migration failed"
            );
        }
        true
    }
}

/// Rebalances the current task at a caller-provided migration-safe point.
///
/// Returns `true` when the task moved to another CPU. Kernel code must not
/// call this while holding a lock, a preemption guard, or other CPU-local
/// state across the call.
pub fn rebalance_current() -> bool {
    #[cfg(not(feature = "smp"))]
    {
        false
    }

    #[cfg(feature = "smp")]
    {
        let curr = current().clone();
        let Some(target_cpu) = crate::run_queue::rebalance_target(&curr) else {
            return false;
        };

        const MIGRATION_TASK_STACK_SIZE: usize = 4096;
        let migration_task = TaskInner::new(
            move || crate::run_queue::migrate_entry_to(curr, target_cpu),
            "rebalance-task".into(),
            MIGRATION_TASK_STACK_SIZE,
        )
        .into_arc();

        current_run_queue::<NoPreemptIrqSave>().migrate_current(migration_task);
        assert_eq!(
            axhal::percpu::this_cpu_id(),
            target_cpu,
            "Rebalancing failed"
        );
        true
    }
}

/// Current task gives up the CPU time voluntarily, and switches to another
/// ready task.
pub fn yield_now() {
    current_run_queue::<NoPreemptIrqSave>().yield_current()
}

/// Requests a reschedule when the current no-preemption section ends.
///
/// Interrupt handlers can use this after waking a latency-sensitive task. The
/// actual context switch is deferred until the interrupt guard is dropped.
pub fn request_reschedule() {
    #[cfg(feature = "preempt")]
    current().set_preempt_pending(true);
}

/// Current task is going to sleep for the given duration.
///
/// If the feature `irq` is not enabled, it uses busy-wait instead.
pub fn sleep(dur: core::time::Duration) {
    sleep_until(axhal::time::wall_time() + dur);
}

/// Current task is going to sleep, it will be woken up at the given deadline.
///
/// If the feature `irq` is not enabled, it uses busy-wait instead.
pub fn sleep_until(deadline: axhal::time::TimeValue) {
    #[cfg(feature = "irq")]
    crate::future::block_on(crate::future::sleep_until(deadline));
    #[cfg(not(feature = "irq"))]
    axhal::time::busy_wait_until(deadline);
}

/// Exits the current task.
pub fn exit(exit_code: i32) -> ! {
    current_run_queue::<NoPreemptIrqSave>().exit_current(exit_code)
}

/// The idle task routine.
///
/// It runs an infinite loop that keeps calling [`yield_now()`].
pub fn run_idle() -> ! {
    loop {
        yield_now();
        trace!("idle task: waiting for IRQs...");
        #[cfg(feature = "irq")]
        axhal::asm::wait_for_irqs();
    }
}
