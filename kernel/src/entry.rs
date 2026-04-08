use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};

use axfs::FS_CONTEXT;
use axhal::uspace::UserContext;
use axsync::Mutex;
use axtask::{AxTaskExt, spawn_task};
use starry_process::{Pid, Process};

use crate::{
    file::FD_TABLE,
    mm::{copy_from_kernel, load_user_app, new_user_aspace_empty},
    pseudofs::{self, dev::tty::N_TTY},
    task::{ProcessData, Thread, add_task_to_table, new_user_task, spawn_alarm_task},
};

fn launch_candidates(args: &[String]) -> Vec<Vec<String>> {
    let mut candidates = vec![args.to_vec()];

    let Some(exec) = args.first().map(String::as_str) else {
        return candidates;
    };

    let busybox_variants = ["/musl/busybox", "/glibc/busybox", "/bin/busybox"];
    if busybox_variants.contains(&exec) {
        for alt in busybox_variants {
            if alt == exec {
                continue;
            }
            let mut candidate = args.to_vec();
            candidate[0] = alt.to_string();
            candidates.push(candidate);
        }
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
                    info!("Init executable fallback: {:?} -> {}", args.first(), candidate[0]);
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
    pseudofs::mount_all().expect("Failed to mount pseudofs");
    spawn_alarm_task();

    let (args, path, name) = resolve_init_command(args);

    let mut uspace = new_user_aspace_empty()
        .and_then(|mut it| {
            copy_from_kernel(&mut it)?;
            Ok(it)
        })
        .expect("Failed to create user address space");

    let (entry_vaddr, ustack_top) = load_user_app(&mut uspace, None, &args, envs)
        .unwrap_or_else(|e| panic!("Failed to load user app: {}", e));

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
        Arc::new(Mutex::new(uspace)),
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

    // TODO: wait for all processes to finish
    let exit_code = task.join();
    info!("Init process exited with code: {exit_code:?}");

    let cx = FS_CONTEXT.lock();
    cx.root_dir()
        .unmount_all()
        .expect("Failed to unmount all filesystems");
    cx.root_dir()
        .filesystem()
        .flush()
        .expect("Failed to flush rootfs");

    info!("Init flow completed, powering off");
    axhal::power::system_off();
}
