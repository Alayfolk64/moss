use axhal::uspace::{ExceptionKind, ReturnReason, UserContext};
use axtask::TaskInner;
use starry_process::Pid;
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr};

use super::{
    AsThread, TimerState, check_signals, do_exit, raise_signal_fatal, set_timer_state,
    unblock_next_signal,
};
use crate::syscall::{handle_syscall, ptrace_wait_if_stopped};

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
                let reason = uctx.run();
                let interrupted = matches!(reason, ReturnReason::Interrupt);

                set_timer_state(&curr, TimerState::Kernel);

                // 让目标线程真正完成退出清理
                if thr.exec_exit_requested() {
                    do_exit(0, false);
                    break;
                }

                match reason {
                    ReturnReason::Syscall => handle_syscall(&mut uctx),
                    ReturnReason::PageFault(addr, flags) => {
                        let fault_signal = {
                            let mut aspace = thr.proc_data.aspace.lock();
                            if crate::syscall::is_mmap_sigbus_fault(
                                thr.proc_data.proc.pid(),
                                addr,
                                flags,
                            ) {
                                Some(Signo::SIGBUS)
                            } else if aspace.handle_page_fault(addr, flags)
                                || crate::syscall::try_growdown_fault(
                                    thr.proc_data.proc.pid(),
                                    &mut aspace,
                                    addr,
                                    flags,
                                )
                            {
                                None
                            } else if aspace.is_accessible_fault(addr, flags) {
                                Some(Signo::SIGBUS)
                            } else {
                                Some(Signo::SIGSEGV)
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
                                if unsafe { uctx.emulate_unaligned() }.is_ok() {
                                    break 'exc;
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

                // do_exit() may have reclaimed the process address space.  Do not
                // touch user memory (including the registered rseq area) afterwards.
                if thr.pending_exit() {
                    break;
                }

                // This is the task's migration-safe point: syscall/trap work
                // is complete, no kernel lock is held, and the user context
                // is fully saved. Rebalance before rseq publishes the CPU ID.
                axtask::rebalance_current();

                let check_signal = !unblock_next_signal();
                let signal_pending =
                    check_signal && !(thr.signal.pending() & !thr.signal.blocked()).is_empty();
                if let Err(err) = thr.prepare_rseq(&mut uctx, interrupted || signal_pending) {
                    warn!("Invalid rseq state: {err:?}");
                    thr.reset_rseq();
                    raise_signal_fatal(SignalInfo::new_kernel(Signo::SIGSEGV))
                        .expect("Failed to send rseq fault signal");
                }

                if check_signal {
                    while check_signals(thr, &mut uctx, None) {}
                }

                set_timer_state(&curr, TimerState::User);
                curr.clear_interrupt();
            }
        },
        name.into(),
        crate::config::KERNEL_STACK_SIZE,
    )
}
