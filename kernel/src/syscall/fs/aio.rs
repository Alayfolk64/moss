use alloc::{
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};
use core::sync::atomic::{AtomicUsize, Ordering};

use axerrno::{AxError, AxResult, LinuxError};
use axtask::current;
use bytemuck::AnyBitPattern;
use lazy_static::lazy_static;
use linux_raw_sys::general::{O_ACCMODE, O_RDONLY, O_WRONLY};
use spin::Mutex;
use starry_process::Pid;
use starry_vm::{VmMutPtr, VmPtr};

use crate::{
    file::{File, FileLike},
    mm::{VmBytes, VmBytesMut},
    task::AsThread,
};

const AIO_MAX_NR: usize = 1024;
const IOCB_CMD_PREAD: u16 = 0;
const IOCB_CMD_PWRITE: u16 = 1;

#[repr(C)]
#[derive(Clone, Copy, AnyBitPattern)]
pub(crate) struct AioIocb {
    aio_data: u64,
    aio_key: u32,
    aio_rw_flags: u32,
    aio_lio_opcode: u16,
    aio_reqprio: i16,
    aio_fildes: u32,
    aio_buf: u64,
    aio_nbytes: u64,
    aio_offset: i64,
    aio_reserved2: u64,
    aio_flags: u32,
    aio_resfd: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct AioEvent {
    data: u64,
    obj: u64,
    res: i64,
    res2: i64,
}

struct AioContext {
    owner_pid: Pid,
    max_events: usize,
    events: VecDeque<AioEvent>,
}

lazy_static! {
    static ref AIO_CONTEXTS: Mutex<BTreeMap<u64, AioContext>> = Mutex::new(BTreeMap::new());
}

static NEXT_AIO_CTX: AtomicUsize = AtomicUsize::new(1);

fn current_pid() -> Pid {
    current().as_thread().proc_data.proc.pid()
}

fn current_context_mut(
    contexts: &mut BTreeMap<u64, AioContext>,
    ctx: u64,
) -> AxResult<&mut AioContext> {
    let aio = contexts.get_mut(&ctx).ok_or(AxError::InvalidInput)?;
    if aio.owner_pid != current_pid() {
        return Err(AxError::InvalidInput);
    }
    Ok(aio)
}

pub fn release_process_aio_resources(pid: Pid) {
    AIO_CONTEXTS
        .lock()
        .retain(|_, context| context.owner_pid != pid);
}

fn aio_err(err: AxError) -> i64 {
    -LinuxError::from(err).code() as i64
}

pub fn sys_io_setup(nr_events: isize, ctxp: *mut u64) -> AxResult<isize> {
    if ctxp.is_null() {
        return Err(AxError::BadAddress);
    }
    if ctxp.vm_read()? != 0 || nr_events <= 0 {
        return Err(AxError::InvalidInput);
    }

    let nr_events = usize::try_from(nr_events).map_err(|_| AxError::InvalidInput)?;
    let mut contexts = AIO_CONTEXTS.lock();
    let used = contexts.values().map(|ctx| ctx.max_events).sum::<usize>();
    if nr_events > AIO_MAX_NR || used + nr_events > AIO_MAX_NR {
        return Err(AxError::WouldBlock);
    }

    let id = NEXT_AIO_CTX.fetch_add(1, Ordering::Relaxed) as u64;
    contexts.insert(
        id,
        AioContext {
            owner_pid: current_pid(),
            max_events: nr_events,
            events: VecDeque::new(),
        },
    );
    ctxp.vm_write(id)?;
    Ok(0)
}

pub fn sys_io_destroy(ctx: u64) -> AxResult<isize> {
    let mut contexts = AIO_CONTEXTS.lock();
    current_context_mut(&mut contexts, ctx)?;
    contexts.remove(&ctx);
    Ok(0)
}

fn run_iocb(iocb: AioIocb) -> AxResult<isize> {
    if iocb.aio_offset < 0 {
        return Err(AxError::InvalidInput);
    }

    let file = File::from_fd(iocb.aio_fildes as i32)?;
    let mode = file.status_flags() & O_ACCMODE;
    let len = usize::try_from(iocb.aio_nbytes).map_err(|_| AxError::InvalidInput)?;
    match iocb.aio_lio_opcode {
        IOCB_CMD_PREAD => {
            if mode == O_WRONLY {
                return Err(AxError::BadFileDescriptor);
            }
            file.inner()
                .read_at(
                    VmBytesMut::new(iocb.aio_buf as *mut u8, len),
                    iocb.aio_offset as _,
                )
                .map(|n| n as _)
        }
        IOCB_CMD_PWRITE => {
            if mode == O_RDONLY {
                return Err(AxError::BadFileDescriptor);
            }
            file.inner()
                .write_at(
                    VmBytes::new(iocb.aio_buf as *const u8, len),
                    iocb.aio_offset as _,
                )
                .map(|n| n as _)
        }
        _ => Err(AxError::InvalidInput),
    }
}

pub fn sys_io_submit(ctx: u64, nr: isize, iocbpp: *const *const AioIocb) -> AxResult<isize> {
    let mut contexts = AIO_CONTEXTS.lock();
    let aio = current_context_mut(&mut contexts, ctx)?;
    if nr < 0 {
        return Err(AxError::InvalidInput);
    }
    let nr = nr as usize;
    if nr == 0 {
        return Ok(0);
    }

    let mut events = Vec::new();
    for i in 0..nr {
        let iocb_ptr = iocbpp.wrapping_add(i).vm_read()?;
        if iocb_ptr.is_null() {
            return Err(AxError::BadAddress);
        }
        let iocb = iocb_ptr.vm_read()?;
        let res = run_iocb(iocb)?;
        events.push(AioEvent {
            data: iocb.aio_data,
            obj: iocb_ptr as u64,
            res: res as _,
            res2: 0,
        });
    }

    if aio.events.len() + events.len() > aio.max_events {
        return Err(AxError::WouldBlock);
    }
    aio.events.extend(events);
    Ok(nr as _)
}

pub fn sys_io_getevents(
    ctx: u64,
    min_nr: isize,
    nr: isize,
    events: *mut AioEvent,
    _timeout: *const (),
) -> AxResult<isize> {
    let mut contexts = AIO_CONTEXTS.lock();
    let aio = current_context_mut(&mut contexts, ctx)?;
    if min_nr < 0 || nr < min_nr {
        return Err(AxError::InvalidInput);
    }
    let nr = nr as usize;
    if nr > 0 && events.is_null() {
        return Err(AxError::BadAddress);
    }

    let mut copied = 0;
    while copied < nr {
        let Some(event) = aio.events.pop_front() else {
            break;
        };
        events.wrapping_add(copied).vm_write(event)?;
        copied += 1;
    }
    Ok(copied as _)
}

pub fn sys_io_cancel(ctx: u64, iocb: *const AioIocb, result: *mut AioEvent) -> AxResult<isize> {
    if iocb.is_null() || result.is_null() {
        return Err(AxError::BadAddress);
    }
    let mut contexts = AIO_CONTEXTS.lock();
    let aio = current_context_mut(&mut contexts, ctx)?;
    let obj = iocb as u64;
    let index = aio
        .events
        .iter()
        .position(|event| event.obj == obj)
        .ok_or_else(|| AxError::from(LinuxError::ECANCELED))?;
    let event = aio.events.remove(index).unwrap_or(AioEvent {
        data: 0,
        obj,
        res: aio_err(AxError::from(LinuxError::ECANCELED)),
        res2: 0,
    });
    result.vm_write(event)?;
    Ok(0)
}
