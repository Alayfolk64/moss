use core::{
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
};

use axerrno::{AxError, AxResult};
use axhal::uspace::UserContext;
use starry_vm::{VmMutPtr, VmPtr};

use super::Thread;

pub const RSEQ_ABI_SIZE: u32 = 32;
pub const RSEQ_FLAG_UNREGISTER: u32 = 1;

const RSEQ_CPU_ID_UNINITIALIZED: u32 = u32::MAX;
static INVALID_LOGS: AtomicUsize = AtomicUsize::new(0);

fn invalid(details: fmt::Arguments<'_>) -> AxError {
    if INVALID_LOGS.fetch_add(1, Ordering::Relaxed) < 16 {
        warn!("[rseq] {details}");
    }
    AxError::BadAddress
}

#[derive(Clone, Copy)]
pub(super) struct RseqRegistration {
    address: usize,
    length: u32,
    signature: u32,
    schedule_count: u64,
    abort_generation: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RseqAbi {
    cpu_id_start: u32,
    cpu_id: u32,
    rseq_cs: u64,
    flags: u32,
    node_id: u32,
    mm_cid: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RseqCriticalSection {
    version: u32,
    flags: u32,
    start_ip: u64,
    post_commit_offset: u64,
    abort_ip: u64,
}

fn read_uninit<T>(pointer: *const T) -> AxResult<T> {
    let value = pointer.vm_read_uninit()?;
    Ok(unsafe { value.assume_init() })
}

fn write_ids(address: usize, cpu_id: u32) -> AxResult {
    (address as *mut u32).vm_write(cpu_id)?;
    ((address + 4) as *mut u32).vm_write(cpu_id)?;
    ((address + 20) as *mut u32).vm_write(0)?;
    // A CPU ID is also a valid per-address-space concurrency ID: only one
    // thread from an address space can execute on a given CPU at a time.
    ((address + 24) as *mut u32).vm_write(cpu_id)?;
    Ok(())
}

fn clear_critical_section(address: usize) -> AxResult {
    ((address + 8) as *mut u64).vm_write(0)?;
    Ok(())
}

impl Thread {
    pub fn register_rseq(&self, address: usize, length: u32, signature: u32) -> AxResult {
        if address == 0 || !address.is_multiple_of(RSEQ_ABI_SIZE as usize) {
            return Err(AxError::InvalidInput);
        }
        if length < RSEQ_ABI_SIZE || address.checked_add(length as usize).is_none() {
            return Err(AxError::InvalidInput);
        }
        if self.rseq.lock().is_some() {
            return Err(AxError::ResourceBusy);
        }

        let abi: RseqAbi = read_uninit(address as *const RseqAbi)?;
        if abi.flags != 0 || abi.rseq_cs != 0 {
            return Err(AxError::InvalidInput);
        }

        let task = axtask::current();
        let cpu_id = axhal::percpu::this_cpu_id() as u32;
        write_ids(address, cpu_id)?;
        *self.rseq.lock() = Some(RseqRegistration {
            address,
            length,
            signature,
            schedule_count: task.schedule_count(),
            abort_generation: self.proc_data.rseq_abort_generation(),
        });
        Ok(())
    }

    pub fn unregister_rseq(&self, address: usize, length: u32, signature: u32) -> AxResult {
        let registration = self.rseq.lock().ok_or(AxError::InvalidInput)?;
        if registration.address != address
            || registration.length != length
            || registration.signature != signature
        {
            return Err(AxError::InvalidInput);
        }

        write_ids(address, RSEQ_CPU_ID_UNINITIALIZED)?;
        clear_critical_section(address)?;
        *self.rseq.lock() = None;
        Ok(())
    }

    pub fn reset_rseq(&self) {
        *self.rseq.lock() = None;
    }

    pub fn inherit_rseq_from(&self, parent: &Thread) {
        *self.rseq.lock() = *parent.rseq.lock();
    }

    /// Updates the user ABI before returning to user mode and restarts an
    /// active critical section after scheduling, migration, or signal work.
    pub fn prepare_rseq(&self, uctx: &mut UserContext, force_abort: bool) -> AxResult {
        let Some(mut registration) = *self.rseq.lock() else {
            return Ok(());
        };

        let task = axtask::current();
        let schedule_count = task.schedule_count();
        let abort_generation = self.proc_data.rseq_abort_generation();
        let cpu_id = axhal::percpu::this_cpu_id() as u32;
        let event = force_abort
            || registration.schedule_count != schedule_count
            || registration.abort_generation != abort_generation;
        if !event {
            return Ok(());
        }

        write_ids(registration.address, cpu_id)?;
        let abi: RseqAbi = read_uninit(registration.address as *const RseqAbi).map_err(|err| {
            invalid(format_args!(
                "cannot read ABI at {:#x}: {err:?}",
                registration.address
            ))
        })?;
        if abi.flags != 0 {
            return Err(invalid(format_args!(
                "ABI flags={:#x} at {:#x}",
                abi.flags, registration.address
            )));
        }

        if abi.rseq_cs != 0 {
            let critical: RseqCriticalSection =
                read_uninit(abi.rseq_cs as usize as *const RseqCriticalSection).map_err(|err| {
                    invalid(format_args!(
                        "cannot read CS at {:#x}, ABI={:#x}: {err:?}",
                        abi.rseq_cs, registration.address
                    ))
                })?;
            if critical.version != 0 || critical.flags != 0 {
                return Err(invalid(format_args!(
                    "CS header version={} flags={:#x} at {:#x}",
                    critical.version, critical.flags, abi.rseq_cs
                )));
            }

            let start = usize::try_from(critical.start_ip)
                .map_err(|_| invalid(format_args!("CS start does not fit usize")))?;
            let length = usize::try_from(critical.post_commit_offset)
                .map_err(|_| invalid(format_args!("CS length does not fit usize")))?;
            let end = start.checked_add(length).ok_or_else(|| {
                invalid(format_args!(
                    "CS range overflows: start={start:#x} length={length:#x}"
                ))
            })?;

            clear_critical_section(registration.address)?;
            if (start..end).contains(&uctx.ip()) {
                let abort_ip = usize::try_from(critical.abort_ip)
                    .map_err(|_| invalid(format_args!("abort IP does not fit usize")))?;
                if abort_ip < 4 || (start..end).contains(&abort_ip) {
                    return Err(invalid(format_args!(
                        "invalid abort IP={abort_ip:#x}, CS={start:#x}..{end:#x}"
                    )));
                }
                let signature = read_uninit((abort_ip - 4) as *const u32).map_err(|err| {
                    invalid(format_args!(
                        "cannot read signature at {:#x}: {err:?}",
                        abort_ip - 4
                    ))
                })?;
                if signature != registration.signature {
                    return Err(invalid(format_args!(
                        "signature mismatch at {:#x}: got={signature:#010x} expected={:#010x}, \
                         IP={:#x}, CS={start:#x}..{end:#x}",
                        abort_ip - 4,
                        registration.signature,
                        uctx.ip(),
                    )));
                }
                uctx.set_ip(abort_ip);
            }
        }

        registration.schedule_count = schedule_count;
        registration.abort_generation = abort_generation;
        *self.rseq.lock() = Some(registration);
        Ok(())
    }
}
