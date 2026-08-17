use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};

use axfs::{
    FS_CONTEXT, enable_async_block_io, shutdown_background_writeback,
    start_background_writeback,
};
use axhal::uspace::UserContext;
use axtask::{AxTaskExt, spawn_task};
use starry_process::{Pid, Process};
use starry_signal::Signo;

use crate::{
    file::FD_TABLE,
    mm::{AddrSpaceLock, copy_from_kernel, load_user_app, new_user_aspace_empty},
    pseudofs::{self, dev::tty::N_TTY},
    task::{ProcessData, Thread, add_task_to_table, new_user_task, spawn_alarm_task},
};

#[inline]
fn boot_progress(_stage: &str) {
    #[cfg(feature = "watchdog")]
    crate::watchdog::note_progress();
}

fn launch_candidates(args: &[String]) -> Vec<Vec<String>> {
    let mut candidates = vec![args.to_vec()];

    let Some(exec) = args.first().map(String::as_str) else {
        return candidates;
    };

    let busybox_variants = ["/musl/busybox", "/glibc/busybox", "/bin/busybox"];
    match exec {
        "/bin/sh" => {
            for busybox in busybox_variants {
                let mut candidate = vec![busybox.to_string(), "sh".to_string()];
                candidate.extend(args[1..].iter().cloned());
                candidates.push(candidate);
            }
        }
        exec if busybox_variants.contains(&exec) => {
            for alt in busybox_variants {
                if alt == exec {
                    continue;
                }
                let mut candidate = args.to_vec();
                candidate[0] = alt.to_string();
                candidates.push(candidate);
            }

            if args.get(1).map(String::as_str) == Some("sh") {
                let mut candidate = vec!["/bin/sh".to_string()];
                candidate.extend(args[2..].iter().cloned());
                candidates.push(candidate);
            }
        }
        _ => {}
    }

    // The final test script only needs a POSIX shell.  Some minimal rootfs
    // images intentionally provide BusyBox sh but no GNU Bash, so a missing
    // /bin/bash is a recoverable rootfs capability difference, not a kernel
    // invariant that should panic during boot.
    if exec == "/bin/bash" {
        let mut candidate = args.to_vec();
        candidate[0] = "/bin/sh".to_string();
        candidates.push(candidate);
    }

    candidates
}

fn resolve_init_command(args: &[String]) -> (Vec<String>, String, String) {
    let mut last_err = None;

    for candidate in launch_candidates(args) {
        match FS_CONTEXT.lock().resolve(&candidate[0]) {
            Ok(loc) => {
                let path = loc
                    .absolute_path()
                    .expect("Failed to get executable absolute path")
                    .to_string();
                let name = loc.name().to_string();
                if candidate != args {
                    info!(
                        "Init executable fallback: {:?} -> {}",
                        args.first(),
                        candidate[0]
                    );
                }
                return (candidate, path, name);
            }
            Err(err) => {
                last_err = Some((candidate[0].clone(), err));
            }
        }
    }

    panic!(
        "Failed to resolve executable path from {:?}, last error: {:?}",
        args.first(),
        last_err
    );
}

/// Initialize and run initproc.
pub fn init(args: &[String], envs: &[String]) {
    // The allocator and physical RAM map are ready. Build the compact COW
    // reference index before the first user address space allocates a frame.
    crate::mm::init_frame_ref_table();

    // Rootfs is mounted before all run queues are online. From this point on,
    // block completions may safely execute in their own task.
    enable_async_block_io();
    start_background_writeback();

    #[cfg(feature = "watchdog")]
    crate::watchdog::spawn_monitor();

    #[cfg(any(target_arch = "loongarch64", target_arch = "riscv64"))]
    crate::mm::init_tlb_shootdown();
    boot_progress("tlb-shootdown-ready");

    #[cfg(feature = "ls2k1000")]
    crate::swap::init();
    boot_progress("swap-ready");

    pseudofs::mount_all().expect("Failed to mount pseudofs");
    boot_progress("pseudofs-ready");
    spawn_alarm_task();
    boot_progress("alarm-task-spawned");

    let (args, path, name) = resolve_init_command(args);
    boot_progress("init-command-resolved");

    let mut uspace = new_user_aspace_empty()
        .and_then(|mut it| {
            copy_from_kernel(&mut it)?;
            Ok(it)
        })
        .expect("Failed to create user address space");
    boot_progress("user-address-space-ready");

    let (entry_vaddr, ustack_top, auxv) = load_user_app(&mut uspace, None, &args, envs)
        .unwrap_or_else(|e| panic!("Failed to load user app: {}", e));
    boot_progress("user-image-loaded");

    let uctx = UserContext::new(entry_vaddr.into(), ustack_top, 0);
    let mut task = new_user_task(&name, uctx, 0);
    task.ctx_mut().set_page_table_root(uspace.page_table_root());

    let pid = task.id().as_u64() as Pid;
    let proc = Process::new_init(pid);
    proc.add_thread(pid);

    N_TTY.bind_to(&proc).expect("Failed to bind ntty");

    let proc = ProcessData::new(
        proc,
        path,
        Arc::new(args),
        auxv,
        Arc::new(AddrSpaceLock::new(uspace)),
        Arc::default(),
        None,
    );

    {
        let mut scope = proc.scope.write();
        crate::file::add_stdio(&mut FD_TABLE.scope_mut(&mut scope).write())
            .expect("Failed to add stdio");
    }

    let thr = Thread::new(pid, proc);
    *task.task_ext_mut() = Some(unsafe { AxTaskExt::from_impl(thr) });

    let task = spawn_task(task);
    add_task_to_table(&task);
    boot_progress("init-task-spawned");

    // TODO: wait for all processes to finish
    boot_progress("init-task-join");
    let exit_code = task.join();
    info!("Init process exited with code: {exit_code:?}");
    #[cfg(feature = "watchdog")]
    crate::watchdog::note_progress();

    // File close no longer forces synchronous writeback. Flush dirty folios
    // while every mount is still reachable, then let unmount and the final
    // filesystem flush drain metadata and device queues.
    shutdown_background_writeback().expect("Failed to sync file caches");
    let cx = FS_CONTEXT.lock();
    cx.root_dir()
        .unmount_all()
        .expect("Failed to unmount all filesystems");
    #[cfg(feature = "watchdog")]
    crate::watchdog::note_progress();
    cx.root_dir()
        .filesystem()
        .shutdown()
        .expect("Failed to finalize rootfs");
    #[cfg(feature = "watchdog")]
    crate::watchdog::note_progress();

    // BusyBox asks init to shut down by signal unless `-f` is used:
    // SIGUSR2 requests power-off and SIGTERM requests reboot.  Preserve the
    // distinction after the init task exits so every platform reaches the
    // corresponding hardware operation after filesystems have been flushed.
    if exit_code == Signo::SIGUSR2 as i32 {
        #[cfg(feature = "watchdog")]
        crate::watchdog::stop();
        axhal::power::system_off();
    }
    if exit_code == Signo::SIGTERM as i32 {
        #[cfg(feature = "watchdog")]
        crate::watchdog::stop();
        axhal::power::system_reset();
    }

    #[cfg(feature = "watchdog")]
    {
        info!("Init flow completed, rebooting for the next automated run");
        crate::watchdog::stop();
        axhal::power::system_reset();
    }
    #[cfg(not(feature = "watchdog"))]
    {
        info!("Init flow completed, powering off");
        axhal::power::system_off();
    }
}
