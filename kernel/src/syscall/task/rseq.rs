use axerrno::{AxError, AxResult};
use axtask::current;

use crate::task::{AsThread, RSEQ_FLAG_UNREGISTER};

pub fn sys_rseq(address: usize, length: u32, flags: u32, signature: u32) -> AxResult<isize> {
    let thread = current();
    let thread = thread.as_thread();

    match flags {
        0 => thread.register_rseq(address, length, signature)?,
        RSEQ_FLAG_UNREGISTER => thread.unregister_rseq(address, length, signature)?,
        _ => return Err(AxError::InvalidInput),
    }
    Ok(0)
}
