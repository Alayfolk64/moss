use alloc::{sync::Arc, vec::Vec};
use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};

use axerrno::{AxError, AxResult, LinuxError};
use axpoll::PollSet;
use axtask::{
    current,
    future::{block_on, interruptible, timeout},
};
use linux_raw_sys::general::{
    FUTEX_32, FUTEX_CLOCK_REALTIME, FUTEX_CMD_MASK, FUTEX_CMP_REQUEUE, FUTEX_PRIVATE_FLAG,
    FUTEX_REQUEUE, FUTEX_WAIT, FUTEX_WAIT_BITSET, FUTEX_WAKE, FUTEX_WAKE_BITSET, robust_list_head,
    timespec,
};
use starry_process::{Pid, init_proc};
use starry_vm::{VmMutPtr, VmPtr};

use crate::{
    task::{AsThread, FutexKey, ProcessData, futex_table_for, get_process_data, get_task},
    time::TimeValueLike,
};

fn assert_unsigned(value: u32) -> AxResult<u32> {
    if (value as i32) < 0 {
        Err(AxError::InvalidInput)
    } else {
        Ok(value)
    }
}

const FUTEX_WAITV_MAX: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FutexWaitv {
    pub val: u64,
    pub uaddr: u64,
    pub flags: u32,
    pub reserved: u32,
}

struct WaitItem {
    uaddr: *const u32,
    expected: u32,
    key: FutexKey,
}

fn unregister_waitv(prepared: &[WaitItem], notifieds: &[Arc<AtomicBool>]) {
    for (item, notified) in prepared.iter().zip(notifieds.iter()) {
        let table = futex_table_for(&item.key);

        if let Some(futex) = table.get(&item.key) {
            futex.wq.unregister_waiter(notified);
        }
    }
}

pub fn sys_futex(
    uaddr: *const u32,
    futex_op: u32,
    value: u32,
    timeout: *const timespec,
    uaddr2: *mut u32,
    value3: u32,
) -> AxResult<isize> {
    debug!(
        "sys_futex <= uaddr: {uaddr:?}, futex_op: {futex_op}, value: {value}, uaddr2: {uaddr2:?}, \
         value3: {value3}",
    );

    let key = FutexKey::new_current(uaddr.addr());

    let futex_table = futex_table_for(&key);

    let command = futex_op & (FUTEX_CMD_MASK as u32);
    match command {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            // Fast path
            if uaddr.vm_read()? != value {
                return Err(AxError::WouldBlock);
            }

            let timeout = if let Some(ts) = timeout.nullable() {
                // FIXME: AnyBitPattern
                let ts = unsafe { ts.vm_read_uninit()?.assume_init() }.try_into_time_value()?;
                if command == FUTEX_WAIT_BITSET {
                    let clock = if futex_op & FUTEX_CLOCK_REALTIME != 0 {
                        axhal::time::wall_time
                    } else {
                        axhal::time::monotonic_time
                    };
                    Some(ts.saturating_sub(clock()))
                } else {
                    Some(ts)
                }
            } else {
                None
            };

            let futex = futex_table.get_or_insert(&key);

            let bitset = if command == FUTEX_WAIT_BITSET {
                value3
            } else {
                u32::MAX
            };

            if !futex
                .wq
                .wait_if(bitset, timeout, || uaddr.vm_read() == Ok(value))?
            {
                return Err(AxError::WouldBlock);
            }

            if futex.owner_dead.swap(false, Ordering::SeqCst) {
                Err(AxError::from(LinuxError::EOWNERDEAD))
            } else {
                Ok(0)
            }
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            let futex = futex_table.get(&key);
            let mut count = 0;
            if let Some(futex) = futex {
                let bitset = if command == FUTEX_WAKE_BITSET {
                    value3
                } else {
                    u32::MAX
                };
                count = futex.wq.wake(value as _, bitset);
            }
            // On SMP, the waiter normally belongs to another run queue, so
            // yielding this CPU only disturbs an unrelated runnable task.
            // Keep the immediate handoff behavior for a single-CPU guest.
            if count != 0 && axhal::cpu_num() == 1 {
                axtask::yield_now();
            }
            Ok(count as _)
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            assert_unsigned(value)?;
            if command == FUTEX_CMP_REQUEUE && uaddr.vm_read()? != value3 {
                return Err(AxError::WouldBlock);
            }
            let value2 = assert_unsigned(timeout.addr() as u32)?;

            let futex = futex_table.get(&key);
            let key2 = FutexKey::new_current(uaddr2.addr());
            let table2 = futex_table_for(&key2);
            let futex2 = table2.get_or_insert(&key2);

            let mut count = 0;
            if let Some(futex) = futex {
                count = futex.wq.wake_requeue(value as _, value2 as _, &futex2.wq);
            }
            Ok(count as _)
        }
        _ => Err(AxError::Unsupported),
    }
}

pub fn sys_futex_waitv(
    waiters: *const FutexWaitv,
    nr_futexs: u32,
    flags: u32,
    timeout_ptr: *const timespec,
    clockid: i32,
) -> AxResult<isize> {
    debug!(
        "sys_futex_waitv <= waiters: {waiters:?}, nr_futexs: {nr_futexs}, flags: {flags}, \
         timeout: {timeout_ptr:?}, clockid: {clockid}"
    );
    if nr_futexs == 0 || nr_futexs as usize > FUTEX_WAITV_MAX {
        return Err(AxError::InvalidInput);
    }

    if flags != 0 {
        return Err(AxError::InvalidInput);
    }

    let mut items = Vec::with_capacity(nr_futexs as usize);

    for i in 0..nr_futexs as usize {
        let ptr = unsafe { waiters.add(i) };

        let item = ptr.vm_read()?;

        if item.reserved != 0 {
            return Err(AxError::InvalidInput);
        }

        items.push(item);
    }

    let mut prepared = Vec::with_capacity(items.len());

    for item in &items {
        let allowed_flags = FUTEX_32 | FUTEX_PRIVATE_FLAG;

        // 必须是 32-bit futex
        if item.flags & FUTEX_32 == 0 {
            return Err(AxError::InvalidInput);
        }

        // 不认识的 item 一律拒绝
        if item.flags & !allowed_flags != 0 {
            return Err(AxError::InvalidInput);
        }

        // u32 futex 必须是 4 字节对齐
        if item.uaddr & 0x03 != 0 {
            return Err(AxError::InvalidInput);
        }

        // 当前只支持 u32,不能静默截断 val
        if item.val > u32::MAX as u64 {
            return Err(AxError::InvalidInput);
        }

        let uaddr = item.uaddr as usize as *const u32;

        let expected = item.val as u32;

        let actual = uaddr.vm_read()?;

        if actual != expected {
            return Err(AxError::WouldBlock);
        }

        let key = if item.flags & FUTEX_PRIVATE_FLAG != 0 {
            FutexKey::Private {
                address: uaddr.addr(),
            }
        } else {
            FutexKey::new_current(uaddr.addr())
        };

        prepared.push(WaitItem {
            uaddr,
            expected,
            key,
        });
    }

    let timeout_dur = if let Some(ts) = timeout_ptr.nullable() {
        let ts = unsafe { ts.vm_read_uninit()?.assume_init() }.try_into_time_value()?;

        let now = match clockid as u32 {
            linux_raw_sys::general::CLOCK_MONOTONIC => axhal::time::monotonic_time(),

            linux_raw_sys::general::CLOCK_REALTIME => axhal::time::wall_time(),

            _ => {
                return Err(AxError::InvalidInput);
            }
        };
        Some(ts.saturating_sub(now))
    } else {
        None
    };
    let poll = Arc::new(PollSet::new());

    let mut notifieds: Vec<Arc<AtomicBool>> = Vec::with_capacity(prepared.len());

    for item in &prepared {
        let table = futex_table_for(&item.key);

        let futex = table.get_or_insert(&item.key);

        let notified = Arc::new(AtomicBool::new(false));
        let registered = futex
            .wq
            .register_waiter(u32::MAX, notified.clone(), poll.clone(), || {
                item.uaddr.vm_read() == Ok(item.expected)
            });

        if !registered {
            unregister_waitv(&prepared, &notifieds);
            return Err(AxError::WouldBlock);
        }
        notifieds.push(notified);
    }

    let result = block_on(interruptible(timeout(
        timeout_dur,
        poll_fn(|cx| {
            poll.register(cx.waker());
            for (index, notified) in notifieds.iter().enumerate() {
                if notified.load(Ordering::Acquire) {
                    return Poll::Ready(index);
                }
            }
            Poll::Pending
        }),
    )));
    unregister_waitv(&prepared, &notifieds);
    match result {
        Ok(Ok(index)) => Ok(index as isize),
        Ok(Err(_)) => Err(AxError::TimedOut),
        Err(_) => Err(AxError::Interrupted),
    }
}

pub fn sys_get_robust_list(
    tid: u32,
    head: *mut *const robust_list_head,
    size: *mut usize,
) -> AxResult<isize> {
    let tid = resolve_robust_list_tid(tid);
    let task = get_task(tid)?;
    check_get_robust_list_permission(
        &current().as_thread().proc_data,
        &task.as_thread().proc_data,
    )?;
    head.vm_write(task.as_thread().robust_list_head() as _)?;
    size.vm_write(size_of::<robust_list_head>())?;

    Ok(0)
}

fn resolve_robust_list_tid(tid: Pid) -> Pid {
    if tid == 1 && get_process_data(1).is_err() {
        init_proc().pid()
    } else {
        tid
    }
}

fn check_get_robust_list_permission(caller: &ProcessData, target: &ProcessData) -> AxResult<()> {
    if caller.proc.pid() == target.proc.pid()
        || caller.euid() == 0
        || caller.uid() == target.uid()
        || caller.uid() == target.suid()
        || caller.euid() == target.uid()
        || caller.euid() == target.suid()
    {
        Ok(())
    } else {
        Err(AxError::from(LinuxError::EPERM))
    }
}

pub fn sys_set_robust_list(head: *const robust_list_head, size: usize) -> AxResult<isize> {
    if size != size_of::<robust_list_head>() {
        return Err(AxError::InvalidInput);
    }
    current().as_thread().set_robust_list_head(head.addr());

    Ok(0)
}
