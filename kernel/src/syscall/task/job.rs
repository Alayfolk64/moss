use axerrno::{AxError, AxResult};
use axtask::current;
use starry_process::Pid;

use crate::task::{
    AsThread, add_process_group_to_table, add_session_to_table, get_process_data, get_process_group,
};

pub fn sys_getsid(pid: Pid) -> AxResult<isize> {
    Ok(get_process_data(pid)?.proc.group().session().sid() as _)
}

pub fn sys_setsid() -> AxResult<isize> {
    let curr = current();
    let proc = &curr.as_thread().proc_data.proc;
    if get_process_group(proc.pid()).is_ok() {
        return Err(AxError::OperationNotPermitted);
    }

    if let Some((session, group)) = proc.create_session() {
        add_session_to_table(&session);
        add_process_group_to_table(&group);
        Ok(session.sid() as _)
    } else {
        Ok(proc.pid() as _)
    }
}

pub fn sys_getpgid(pid: Pid) -> AxResult<isize> {
    Ok(get_process_data(pid)?.proc.group().pgid() as _)
}

pub fn sys_setpgid(pid: Pid, pgid: Pid) -> AxResult<isize> {
    let proc = &get_process_data(pid)?.proc;
    let pgid = if pgid == 0 { proc.pid() } else { pgid };

    if pgid == proc.pid() {
        if let Some(group) = proc.create_group() {
            add_process_group_to_table(&group);
        }
    } else if !proc.move_to_group(&get_process_group(pgid)?) {
        return Err(AxError::OperationNotPermitted);
    }

    Ok(0)
}

// TODO: job control
