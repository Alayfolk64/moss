use axhal::uspace::{ExceptionKind, ReturnReason, UserContext};
use axtask::TaskInner;
use starry_process::Pid;
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr};

use super::{
    AsThread, Thread, TimerState, check_signals, do_exit, raise_signal_fatal, set_timer_state,
    unblock_next_signal,
};
use crate::syscall::{handle_syscall, ptrace_wait_if_stopped};

fn exit_if_requested_by_exec(thr: &Thread) -> bool {
    if thr.pending_exit() {
        return true;
    }
    if !thr.exec_exit_requested() {
        return false;
    }
    do_exit(0, false);
    true
}

/// Create a new user task.
pub fn new_user_task(name: &str, mut uctx: UserContext, set_child_tid: usize) -> TaskInner {
    TaskInner::new(
        move || {
            let curr = axtask::current();

            if let Some(tid) = (set_child_tid as *mut Pid).nullable() {
                tid.vm_write(curr.id().as_u64() as Pid).ok();
            }

            info!("Enter user space: ip={:#x}, sp={:#x}", uctx.ip(), uctx.sp());

            let thr = curr.as_thread();
            while !thr.pending_exit() {
                ptrace_wait_if_stopped(thr);
                if exit_if_requested_by_exec(thr) {
                    break;
                }

                let reason = uctx.run();
                let interrupted = matches!(reason, ReturnReason::Interrupt);

                set_timer_state(&curr, TimerState::Kernel);

                if exit_if_requested_by_exec(thr) {
                    break;
                }

                match reason {
                    ReturnReason::Syscall => handle_syscall(&mut uctx),
                    ReturnReason::PageFault(addr, flags) => {
                        let fault_signal = {
                            let aspace_ref = thr.proc_data.aspace();
                            if crate::syscall::is_mmap_sigbus_fault(
                                thr.proc_data.proc.pid(),
                                addr,
                                flags,
                            ) {
                                Some(Signo::SIGBUS)
                            } else if crate::mm::AddrSpace::handle_page_fault_concurrent(
                                &aspace_ref,
                                addr,
                                flags,
                            ) {
                                None
                            } else {
                                let mut aspace = aspace_ref.lock();
                                if crate::syscall::try_growdown_fault(
                                    thr.proc_data.proc.pid(),
                                    &mut aspace,
                                    addr,
                                    flags,
                                ) {
                                    None
                                } else if aspace.is_accessible_fault(addr, flags) {
                                    Some(Signo::SIGBUS)
                                } else {
                                    Some(Signo::SIGSEGV)
                                }
                            }
                        };
                        if let Some(signo) = fault_signal {
                            info!(
                                "{:?}: user fault {:?} at {:#x} {:?}",
                                thr.proc_data.proc, signo, addr, flags
                            );
                            raise_signal_fatal(SignalInfo::new_kernel(signo))
                                .expect("Failed to send fault signal");
                        }
                    }
                    ReturnReason::Interrupt => {}
                    #[allow(unused_labels)]
                    ReturnReason::Exception(exc_info) => 'exc: {
                        // TODO: detailed handling
                        let signo = match exc_info.kind() {
                            ExceptionKind::Misaligned => {
                                #[cfg(target_arch = "loongarch64")]
                                match crate::mm::access_user_memory(|| unsafe {
                                    uctx.emulate_unaligned_at(exc_info.badv as u64, exc_info.badi)
                                }) {
                                    Ok(()) => break 'exc,
                                    Err(error) => warn!(
                                        "failed to emulate user unaligned access: task={} \
                                         era={:#x} badv={:#x} badi={:#010x} exception={:?} \
                                         error={}",
                                        curr.id_name(),
                                        uctx.ip(),
                                        exc_info.badv,
                                        exc_info.badi,
                                        exc_info.e,
                                        error,
                                    ),
                                }
                                Signo::SIGBUS
                            }
                            ExceptionKind::Breakpoint => Signo::SIGTRAP,
                            ExceptionKind::IllegalInstruction => Signo::SIGILL,
                            _ => Signo::SIGTRAP,
                        };
                        raise_signal_fatal(SignalInfo::new_kernel(signo))
                            .expect("Failed to send SIGTRAP");
                    }
                    r => {
                        warn!("Unexpected return reason: {r:?}");
                        raise_signal_fatal(SignalInfo::new_kernel(Signo::SIGSEGV))
                            .expect("Failed to send SIGSEGV");
                    }
                }

                if exit_if_requested_by_exec(thr) {
                    break;
                }

                let check_signal = !unblock_next_signal();
                let signal_pending =
                    check_signal && !(thr.signal.pending() & !thr.signal.blocked()).is_empty();
                // New tasks are already placed on the least-loaded run queue,
                // and an explicit affinity change performs its own migration.
                // Migrating here on every syscall/trap return creates a fresh
                // helper task each time and degenerates into a migration-task
                // storm under syscall-heavy workloads such as buildstorm.
                // rseq observes real scheduling and migration through the
                // task's schedule_count, so no hot-path rebalance is needed.
                if let Err(err) = thr.prepare_rseq(&mut uctx, interrupted || signal_pending) {
                    warn!("Invalid rseq state: {err:?}");
                    thr.reset_rseq();
                    raise_signal_fatal(SignalInfo::new_kernel(Signo::SIGSEGV))
                        .expect("Failed to send rseq fault signal");
                }

                if check_signal {
                    while check_signals(thr, &mut uctx, None) {}
                }

                if exit_if_requested_by_exec(thr) {
                    break;
                }

                curr.clear_interrupt();
                if exit_if_requested_by_exec(thr) {
                    break;
                }
                set_timer_state(&curr, TimerState::User);
            }
        },
        name.into(),
        crate::config::KERNEL_STACK_SIZE,
    )
}
