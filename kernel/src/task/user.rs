use axhal::uspace::{ExceptionKind, ReturnReason, UserContext};
use axtask::TaskInner;
use memory_addr::VirtAddr;
use starry_process::Pid;
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr};

use super::{
    AsThread, TimerState, check_signals, do_exit, raise_signal_fatal, set_timer_state,
    unblock_next_signal,
};
use crate::syscall::{handle_syscall, ptrace_wait_if_stopped};

fn log_user_fault_mapping(aspace: &crate::mm::AddrSpace, label: &str, addr: VirtAddr) {
    if let Some(area) = aspace.find_area(addr) {
        warn!(
            "user fault {label}={addr:#x} in VMA {:#x}-{:#x} {:?}",
            area.start(),
            area.end(),
            area.flags(),
        );
        return;
    }

    let below = aspace
        .areas()
        .filter(|area| area.end() <= addr)
        .max_by_key(|area| area.end());
    let above = aspace
        .areas()
        .filter(|area| area.start() > addr)
        .min_by_key(|area| area.start());
    warn!(
        "user fault {label}={addr:#x} in VMA gap, below={:?}, above={:?}",
        below.map(|area| (area.start(), area.end(), area.flags())),
        above.map(|area| (area.start(), area.end(), area.flags())),
    );
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
                        #[cfg(feature = "fault-repro-trace")]
                        let trace_repro = crate::mm::is_page_fault_repro_address(addr);
                        #[cfg(feature = "fault-repro-trace")]
                        if trace_repro {
                            warn!(
                                "[PFTRACE 05] task-dispatch name={} pid={} tid={} \
                                 ReturnReason::PageFault(addr={addr:#x}, flags={flags:?}) ip={:#x}",
                                curr.name(),
                                thr.proc_data.proc.pid(),
                                curr.id().as_u64(),
                                uctx.ip(),
                            );
                        }
                        let fault_signal = {
                            let aspace_ref = thr.proc_data.aspace();
                            if crate::syscall::is_mmap_sigbus_fault(
                                thr.proc_data.proc.pid(),
                                addr,
                                flags,
                            ) {
                                Some(Signo::SIGBUS)
                            } else {
                                let handled = crate::mm::AddrSpace::handle_page_fault_concurrent(
                                    &aspace_ref,
                                    addr,
                                    flags,
                                );
                                #[cfg(feature = "fault-repro-trace")]
                                if trace_repro && handled {
                                    warn!(
                                        "[PFTRACE 10] fault-handler-return handled=true; \
                                         resume_same_ip={:#x} (faulting store will be retried)",
                                        uctx.ip(),
                                    );
                                }
                                if handled {
                                    None
                                } else {
                                    let mut aspace = aspace_ref
                                        .lock_profiled(crate::mm::AddrSpaceWriteOwner::Other);
                                    if crate::syscall::try_growdown_fault(
                                        thr.proc_data.proc.pid(),
                                        &mut aspace,
                                        addr,
                                        flags,
                                    ) {
                                        None
                                    } else {
                                        let signo = if aspace.is_accessible_fault(addr, flags) {
                                            Signo::SIGBUS
                                        } else {
                                            Signo::SIGSEGV
                                        };
                                        log_user_fault_mapping(&aspace, "address", addr);
                                        log_user_fault_mapping(
                                            &aspace,
                                            "instruction",
                                            VirtAddr::from_usize(uctx.ip()),
                                        );
                                        log_user_fault_mapping(
                                            &aspace,
                                            "stack",
                                            VirtAddr::from_usize(uctx.sp()),
                                        );
                                        Some(signo)
                                    }
                                }
                            }
                        };
                        if let Some(signo) = fault_signal {
                            warn!(
                                "{:?}: user fault {:?} at {:#x} {:?}, ip={:#x}, sp={:#x}",
                                thr.proc_data.proc,
                                signo,
                                addr,
                                flags,
                                uctx.ip(),
                                uctx.sp(),
                            );
                            raise_signal_fatal(SignalInfo::new_kernel(signo))
                                .expect("Failed to send fault signal");
                        }
                    }
                    ReturnReason::Interrupt => {}
                    #[allow(unused_labels)]
                    ReturnReason::Exception(exc_info) => 'exc: {
                        warn!(
                            "{:?}: user exception {:?}, ip={:#x}, sp={:#x}",
                            thr.proc_data.proc,
                            exc_info,
                            uctx.ip(),
                            uctx.sp(),
                        );
                        {
                            let aspace_ref = thr.proc_data.aspace();
                            let aspace = aspace_ref.read();
                            log_user_fault_mapping(
                                &aspace,
                                "instruction",
                                VirtAddr::from_usize(uctx.ip()),
                            );
                            log_user_fault_mapping(
                                &aspace,
                                "stack",
                                VirtAddr::from_usize(uctx.sp()),
                            );
                        }
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

                let check_signal = !unblock_next_signal();
                let signal_pending = check_signal && thr.signal.has_deliverable_signal();
                // Blocking syscalls keep a task on its previous run queue. Rebalance at
                // the user-return boundary, where no syscall lock or CPU-local kernel
                // state is held, so a busy CPU cannot retain all frequently-woken tasks.
                let migrated = axtask::rebalance_current();
                if let Err(err) =
                    thr.prepare_rseq(&mut uctx, interrupted || signal_pending || migrated)
                {
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
