use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::ffi::{c_char, c_int};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, FileFlags};
use axfs_ng_vfs::{Location, NodeType};
use axhal::uspace::UserContext;
use axtask::{TaskState, current, yield_now};
use kernel_guard::NoPreemptIrqSave;
use linux_raw_sys::general::{AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW, MS_NOEXEC};
use starry_process::Pid;
use starry_vm::vm_load_until_nul;

use super::ptrace_on_exec;
use crate::{
    config::USER_HEAP_BASE,
    file::{FD_TABLE, File, resolve_at},
    mm::{
        copy_from_kernel, defer_address_space_reclaim, load_user_app, load_user_app_from_location,
        new_user_aspace_empty, vm_load_string,
    },
    syscall::{
        check_fanotify_open_exec_permission_path, check_fanotify_open_permission_path,
        close_cloexec_files_for_current_process, notify_fanotify_open_exec_path,
        unshare_current_fd_table,
    },
    task::{AsThread, ProcessData, address_space_shared_with_live_process, processes, tasks},
};

struct ExecStateGuard<'a> {
    proc_data: &'a ProcessData,
}

impl Drop for ExecStateGuard<'_> {
    fn drop(&mut self) {
        self.proc_data.set_exec_in_progress(false);
    }
}

fn terminate_other_threads_for_exec(proc_data: &ProcessData, current_tid: Pid) {
    // Hold strong references while clone is excluded. This includes a sibling
    // that removed its TID from Process::threads() but has not switched off
    // its CPU and the old hardware page-table root yet.
    let sibling_tasks = tasks()
        .into_iter()
        .filter(|task| {
            task.id().as_u64() as Pid != current_tid
                && task
                    .try_as_thread()
                    .is_some_and(|thread| core::ptr::eq(thread.proc_data.as_ref(), proc_data))
        })
        .collect::<Vec<_>>();

    loop {
        let mut all_exited = true;
        for task in &sibling_tasks {
            if task.state() != TaskState::Exited {
                all_exited = false;
                let thread = task
                    .try_as_thread()
                    .expect("user task lost its thread state during exec");
                thread.request_exec_exit();
                task.interrupt();
            }
        }
        if all_exited {
            break;
        }
        yield_now();
    }

    // Exited is published before the final context switch. join() also waits
    // for on_cpu=false, acknowledging that the old root is no longer active.
    for task in &sibling_tasks {
        task.join();
    }

    debug_assert!(
        proc_data
            .proc
            .threads()
            .into_iter()
            .all(|tid| tid == current_tid)
    );
}

fn has_exec_permission(loc: &Location, uid: u32, gid: u32) -> AxResult<bool> {
    let meta = loc.metadata()?;
    if meta.node_type != NodeType::RegularFile {
        return Ok(false);
    }

    let mode = meta.mode.bits() as u32;
    if uid == 0 {
        // root still needs at least one execute bit.
        return Ok(mode & 0o111 != 0);
    }
    if uid == meta.uid {
        return Ok(mode & 0o100 != 0);
    }
    if gid == meta.gid {
        return Ok(mode & 0o010 != 0);
    }
    Ok(mode & 0o001 != 0)
}

fn is_text_busy(loc: &Location) -> bool {
    let Ok(target) = loc.metadata() else {
        return false;
    };

    for proc_data in processes() {
        let scope = proc_data.scope.read();
        let fd_scope = FD_TABLE.scope(&scope);
        let fd_table = fd_scope.read();
        for fd in fd_table.ids() {
            let Some(file_desc) = fd_table.get(fd) else {
                continue;
            };
            let Some(file) = file_desc.inner.downcast_ref::<File>() else {
                continue;
            };
            let flags = file.inner().flags();
            if !flags.intersects(FileFlags::WRITE | FileFlags::APPEND) {
                continue;
            }

            let Ok(meta) = file.inner().location().metadata() else {
                continue;
            };
            if meta.device == target.device && meta.inode == target.inode {
                return true;
            }
        }
    }
    false
}

pub fn sys_execve(
    uctx: &mut UserContext,
    path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> AxResult<isize> {
    let path = vm_load_string(path)?;
    let exec_loc = FS_CONTEXT.lock().resolve(&path).ok();
    let load_loc = exec_loc.clone();
    execve_common(uctx, path, exec_loc, load_loc, argv, envp)
}

pub fn sys_execveat(
    uctx: &mut UserContext,
    dirfd: c_int,
    path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
    flags: u32,
) -> AxResult<isize> {
    const SUPPORTED_FLAGS: u32 = AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW;

    if flags & !SUPPORTED_FLAGS != 0 {
        return Err(AxError::InvalidInput);
    }

    let requested_path = vm_load_string(path)?;
    let exec_loc = if requested_path.starts_with('/') {
        let fs = FS_CONTEXT.lock();
        if flags & AT_SYMLINK_NOFOLLOW != 0 {
            fs.resolve_no_follow(&requested_path)?
        } else {
            fs.resolve(&requested_path)?
        }
    } else {
        resolve_at(dirfd, Some(requested_path.as_str()), flags)?
            .into_file()
            .ok_or(AxError::PermissionDenied)?
    };
    if flags & AT_SYMLINK_NOFOLLOW != 0 && exec_loc.metadata()?.node_type == NodeType::Symlink {
        return Err(AxError::FilesystemLoop);
    }

    let path = if requested_path.starts_with('/') {
        requested_path
    } else {
        exec_loc.absolute_path()?.to_string()
    };
    execve_common(
        uctx,
        path,
        Some(exec_loc.clone()),
        Some(exec_loc),
        argv,
        envp,
    )
}

fn execve_common(
    uctx: &mut UserContext,
    path: String,
    exec_loc: Option<Location>,
    load_loc: Option<Location>,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> AxResult<isize> {
    let mut args = if argv.is_null() {
        // Handle NULL argv (treat as empty array)
        Vec::new()
    } else {
        vm_load_until_nul(argv)?
            .into_iter()
            .map(vm_load_string)
            .collect::<Result<Vec<_>, _>>()?
    };
    if args.is_empty() {
        // Linux fills in a dummy argv[0] for empty argv.
        args.push(String::new());
    }

    let envs = if envp.is_null() {
        // Handle NULL envp (treat as empty array)
        Vec::new()
    } else {
        vm_load_until_nul(envp)?
            .into_iter()
            .map(vm_load_string)
            .collect::<Result<Vec<_>, _>>()?
    };

    debug!("sys_execve <= path: {path:?}, args: {args:?}, envs: {envs:?}");

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    if let Some(exec_loc) = &exec_loc {
        if exec_loc.mount_flags() & MS_NOEXEC != 0 {
            return Err(AxError::PermissionDenied);
        }
        if !has_exec_permission(exec_loc, proc_data.euid(), proc_data.egid())? {
            return Err(AxError::PermissionDenied);
        }
        if is_text_busy(exec_loc) {
            return Err(AxError::from(LinuxError::ETXTBSY));
        }
    }

    let current_tid = curr.id().as_u64() as Pid;
    if current_tid != proc_data.proc.pid() {
        warn!(
            "sys_execve: non-leader exec is not supported yet: tid={}, pid={}",
            current_tid,
            proc_data.proc.pid()
        );
        return Err(AxError::WouldBlock);
    }

    let fanotify_exec_path = exec_loc
        .as_ref()
        .and_then(|loc| loc.absolute_path().ok())
        .map_or_else(|| path.clone(), |path| path.to_string());
    check_fanotify_open_permission_path(&fanotify_exec_path)?;
    check_fanotify_open_exec_permission_path(&fanotify_exec_path)?;

    let loc = if let Some(exec_loc) = exec_loc {
        exec_loc
    } else {
        FS_CONTEXT.lock().resolve(&path)?
    };
    let exec_path = loc.absolute_path()?.to_string();

    let mut new_aspace = new_user_aspace_empty()?;
    copy_from_kernel(&mut new_aspace)?;
    let (entry_point, user_stack_base, auxv) = if let Some(load_loc) = load_loc {
        load_user_app_from_location(&mut new_aspace, load_loc, path.as_str(), &args, &envs)?
    } else {
        load_user_app(&mut new_aspace, Some(path.as_str()), &args, &envs)?
    };
    let page_table_root = new_aspace.page_table_root();

    // Loading above is fallible and leaves the running image untouched. Start
    // the irreversible thread-group commit only after the new image is ready.
    {
        let _exec_lock = proc_data.exec_lock.lock();
        if proc_data.exec_in_progress() {
            return Err(AxError::WouldBlock);
        }
        proc_data.set_exec_in_progress(true);
    }
    let _exec_state_guard = ExecStateGuard { proc_data };
    terminate_other_threads_for_exec(proc_data, current_tid);

    // Keep the old address space locked until this CPU has published and
    // activated the replacement root. A CLONE_VM peer cannot reclaim it in
    // the middle of the local commit.
    let old_aspace_snapshot = proc_data.aspace();
    let old_aspace_guard = old_aspace_snapshot.lock();
    let old_aspace = {
        let _commit_guard = NoPreemptIrqSave::new();
        let old_aspace =
            proc_data.replace_aspace(Arc::new(crate::mm::AddrSpaceLock::new(new_aspace)));
        debug_assert!(Arc::ptr_eq(&old_aspace, &old_aspace_snapshot));
        axtask::replace_current_page_table(page_table_root);
        old_aspace
    };
    let old_aspace_shared =
        address_space_shared_with_live_process(&old_aspace, proc_data.proc.pid());
    let old_locked_bytes = if old_aspace_shared {
        0
    } else {
        old_aspace_guard.locked_bytes()
    };
    crate::syscall::release_process_mmap_resources(proc_data.proc.pid(), old_locked_bytes);
    drop(old_aspace_guard);
    drop(old_aspace_snapshot);
    if old_aspace_shared {
        drop(old_aspace);
    } else {
        defer_address_space_reclaim(old_aspace);
    }

    curr.set_name(loc.name());

    notify_fanotify_open_exec_path(&exec_path);
    *proc_data.exe_path.write() = exec_path;
    *proc_data.cmdline.write() = Arc::new(args);
    *proc_data.auxv.write() = auxv;

    proc_data.set_heap_top(USER_HEAP_BASE);

    *proc_data.signal.actions.lock() = Default::default();

    // Clear per-thread user pointers that belong to the old image.
    // They can become dangling once the address space is replaced.
    curr.as_thread().set_clear_child_tid(0);
    curr.as_thread().set_robust_list_head(0);
    curr.as_thread().reset_rseq();

    // A successful exec undoes CLONE_FILES. Detach before applying CLOEXEC so
    // the old table and any pipe endpoints remain owned by the live peer.
    unshare_current_fd_table();
    close_cloexec_files_for_current_process();

    ptrace_on_exec(proc_data.proc.pid());

    uctx.set_ip(entry_point.as_usize());
    uctx.set_sp(user_stack_base.as_usize());
    proc_data.complete_vfork();
    Ok(0)
}
