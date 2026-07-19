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

    while completed.load(Ordering::Acquire) < axconfig::plat::CPU_NUM {
        spin_loop();
    }
}
