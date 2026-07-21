use core::sync::atomic::{Ordering, fence};

use axerrno::{AxError, AxResult, LinuxError};
use axtask::current;

use crate::task::AsThread;

/// Memory barrier commands
const MEMBARRIER_CMD_QUERY: i32 = 0;
const MEMBARRIER_CMD_GLOBAL: i32 = 1;
const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = 2;
const MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED: i32 = 4;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED: i32 = 8;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: i32 = 16;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ: i32 = 128;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: i32 = 256;

const MEMBARRIER_STATE_GLOBAL_EXPEDITED: u32 = MEMBARRIER_CMD_GLOBAL_EXPEDITED as u32;
const MEMBARRIER_STATE_PRIVATE_EXPEDITED: u32 = MEMBARRIER_CMD_PRIVATE_EXPEDITED as u32;
const MEMBARRIER_STATE_PRIVATE_EXPEDITED_RSEQ: u32 = MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ as u32;

/// Supported command flags for query
const SUPPORTED_COMMANDS: i32 = MEMBARRIER_CMD_GLOBAL
    | MEMBARRIER_CMD_GLOBAL_EXPEDITED
    | MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED
    | MEMBARRIER_CMD_PRIVATE_EXPEDITED
    | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
    | MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ
    | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ;

fn smp_mb() {
    fence(Ordering::SeqCst);
}

pub fn sys_membarrier(cmd: i32, flags: u32, _cpu_id: i32) -> AxResult<isize> {
    if flags != 0 {
        return Err(AxError::InvalidInput);
    }

    match cmd {
        MEMBARRIER_CMD_QUERY => Ok(SUPPORTED_COMMANDS as isize),
        MEMBARRIER_CMD_GLOBAL => {
            smp_mb();
            Ok(0)
        }
        MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED => {
            current()
                .as_thread()
                .proc_data
                .add_membarrier_state(MEMBARRIER_STATE_GLOBAL_EXPEDITED);
            Ok(0)
        }
        MEMBARRIER_CMD_GLOBAL_EXPEDITED => {
            let task = current();
            let proc_data = &task.as_thread().proc_data;
            if proc_data.membarrier_state() & MEMBARRIER_STATE_GLOBAL_EXPEDITED == 0 {
                return Err(AxError::from(LinuxError::EPERM));
            }
            smp_mb();
            Ok(0)
        }
        MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
            current()
                .as_thread()
                .proc_data
                .add_membarrier_state(MEMBARRIER_STATE_PRIVATE_EXPEDITED);
            Ok(0)
        }
        MEMBARRIER_CMD_PRIVATE_EXPEDITED => {
            if current().as_thread().proc_data.membarrier_state()
                & MEMBARRIER_STATE_PRIVATE_EXPEDITED
                == 0
            {
                return Err(AxError::from(LinuxError::EPERM));
            }
            smp_mb();
            Ok(0)
        }
        MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ => {
            current()
                .as_thread()
                .proc_data
                .add_membarrier_state(MEMBARRIER_STATE_PRIVATE_EXPEDITED_RSEQ);
            Ok(0)
        }
        MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ => {
            let task = current();
            let proc_data = &task.as_thread().proc_data;
            if proc_data.membarrier_state() & MEMBARRIER_STATE_PRIVATE_EXPEDITED_RSEQ == 0 {
                return Err(AxError::from(LinuxError::EPERM));
            }
            proc_data.request_rseq_abort();
            smp_mb();
            Ok(0)
        }
        _ => Err(AxError::InvalidInput),
    }
}
