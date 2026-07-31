use alloc::sync::Arc;
use core::{
    hint::spin_loop,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

static BROADCAST_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct BroadcastGuard;

impl BroadcastGuard {
    fn acquire() -> Self {
        while BROADCAST_IN_PROGRESS
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
        Self
    }
}

impl Drop for BroadcastGuard {
    fn drop(&mut self) {
        BROADCAST_IN_PROGRESS.store(false, Ordering::Release);
    }
}

/// Runs a callback on the CPUs accepted by `selected` and waits until all
/// selected callbacks have completed.
///
/// The predicate is evaluated synchronously while migration is disabled. A
/// target CPU may leave the selected state after the decision, which only
/// causes a harmless extra callback. Callers that track page-table activity
/// must publish a newly selected state before installing it and flush locally
/// after installation, so a CPU entering after the decision cannot be missed.
pub fn run_on_selected_cpus(
    mut selected: impl FnMut(usize) -> bool,
    callback: impl Fn() + Send + Sync + 'static,
) {
    assert!(
        axhal::asm::irqs_enabled(),
        "synchronous IPI selection requires interrupts to be enabled"
    );

    let _preempt_guard = kernel_guard::NoPreempt::new();
    let _broadcast_guard = BroadcastGuard::acquire();
    let mut targets = [false; axconfig::plat::CPU_NUM];
    let mut target_count = 0;
    for cpu in 0..axhal::cpu_num() {
        if selected(cpu) {
            targets[cpu] = true;
            target_count += 1;
        }
    }
    if target_count == 0 {
        return;
    }

    let callback = Arc::new(callback);
    let completed = Arc::new(AtomicUsize::new(0));
    let remote_completed = completed.clone();
    axipi::run_on_each_cpu(move || {
        let cpu = axhal::percpu::this_cpu_id();
        if targets[cpu] {
            callback();
            remote_completed.fetch_add(1, Ordering::Release);
        }
    });

    while completed.load(Ordering::Acquire) < target_count {
        spin_loop();
    }
}

/// Runs a callback on every CPU and waits until all callbacks have completed.
pub fn run_on_all_cpus(callback: impl Fn() + Send + Sync + 'static) {
    assert!(
        axhal::asm::irqs_enabled(),
        "synchronous IPI broadcast requires interrupts to be enabled"
    );

    // Keep the owner on this CPU, but leave interrupts enabled so concurrent
    // broadcasts from other CPUs can still be serviced while waiting here.
    let _preempt_guard = kernel_guard::NoPreempt::new();
    let _broadcast_guard = BroadcastGuard::acquire();
    let completed = Arc::new(AtomicUsize::new(0));
    let remote_completed = completed.clone();

    axipi::run_on_each_cpu(move || {
        callback();
        remote_completed.fetch_add(1, Ordering::Release);
    });

    while completed.load(Ordering::Acquire) < axhal::cpu_num() {
        spin_loop();
    }
}
