use alloc::{string::{String, ToString}, sync::Arc, vec::Vec};
use core::ffi::c_char;

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, FileFlags};
use axfs_ng_vfs::{Location, NodeType};
use axhal::uspace::UserContext;
use axtask::current;
use starry_vm::vm_load_until_nul;

use crate::{
    config::USER_HEAP_BASE,
    file::{FD_TABLE, File},
    mm::{load_user_app, vm_load_string},
    task::{AsThread, processes},
};

use super::ptrace_on_exec;

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
        if !has_exec_permission(exec_loc, proc_data.euid(), proc_data.egid())? {
            return Err(AxError::PermissionDenied);
        }
        if is_text_busy(exec_loc) {
            return Err(AxError::from(LinuxError::ETXTBSY));
        }
    }

    if proc_data.proc.threads().len() > 1 {
        // TODO: handle multi-thread case
        error!("sys_execve: multi-thread not supported");
        return Err(AxError::WouldBlock);
    }

    let mut aspace = proc_data.aspace.lock();
    let (entry_point, user_stack_base) =
        load_user_app(&mut aspace, Some(path.as_str()), &args, &envs)?;
    drop(aspace);

    let loc = FS_CONTEXT.lock().resolve(&path)?;
    curr.set_name(loc.name());

    *proc_data.exe_path.write() = loc.absolute_path()?.to_string();
    *proc_data.cmdline.write() = Arc::new(args);

    proc_data.set_heap_top(USER_HEAP_BASE);

    *proc_data.signal.actions.lock() = Default::default();

    // Clear per-thread user pointers that belong to the old image.
    // They can become dangling once the address space is replaced.
    curr.as_thread().set_clear_child_tid(0);
    curr.as_thread().set_robust_list_head(0);

    // Close CLOEXEC file descriptors
    let mut fd_table = FD_TABLE.write();
    let cloexec_fds = fd_table
        .ids()
        .filter(|it| fd_table.get(*it).unwrap().cloexec)
        .collect::<Vec<_>>();
    for fd in cloexec_fds {
        fd_table.remove(fd);
    }
    drop(fd_table);

    ptrace_on_exec(proc_data.proc.pid());

    uctx.set_ip(entry_point.as_usize());
    uctx.set_sp(user_stack_base.as_usize());
    Ok(0)
}
