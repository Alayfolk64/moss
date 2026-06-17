use alloc::{format, sync::Arc, vec::Vec};
use core::{ffi::c_char, str, sync::atomic::{AtomicU32, Ordering}};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{FS_CONTEXT, OpenOptions};
use linux_raw_sys::general::{
    F_SEAL_FUTURE_WRITE, F_SEAL_GROW, F_SEAL_SEAL, F_SEAL_SHRINK, F_SEAL_WRITE,
    MFD_ALLOW_SEALING, MFD_CLOEXEC, MFD_HUGETLB, MFD_HUGE_MASK, MFD_HUGE_SHIFT,
};

use crate::{
    file::{File, FileLike},
    mm::UserConstPtr,
};

pub struct MemfdState {
    seals: AtomicU32,
}

impl MemfdState {
    fn new(allow_sealing: bool) -> Self {
        Self {
            seals: AtomicU32::new(if allow_sealing { 0 } else { F_SEAL_SEAL }),
        }
    }

    pub fn seals(&self) -> u32 {
        self.seals.load(Ordering::SeqCst)
    }

    pub fn add_seals(&self, seals: u32) -> AxResult {
        const VALID_SEALS: u32 =
            F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_FUTURE_WRITE;

        if seals & !VALID_SEALS != 0 {
            return Err(AxError::InvalidInput);
        }
        if self.seals() & F_SEAL_SEAL != 0 {
            return Err(AxError::from(LinuxError::EPERM));
        }
        self.seals.fetch_or(seals, Ordering::SeqCst);
        Ok(())
    }
}

pub fn memfd_state(file: &File) -> Option<Arc<MemfdState>> {
    file.inner().location().user_data().get::<MemfdState>()
}

pub fn check_memfd_write(file: &File) -> AxResult {
    let Some(state) = memfd_state(file) else {
        return Ok(());
    };
    let seals = state.seals();
    if seals & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0 {
        Err(AxError::from(LinuxError::EPERM))
    } else {
        Ok(())
    }
}

pub fn memfd_write_sealed(file: &File) -> bool {
    memfd_state(file).is_some_and(|state| {
        state.seals() & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0
    })
}

pub fn check_memfd_write_at(file: &File, offset: u64, len: usize) -> AxResult {
    check_memfd_write(file)?;
    let Some(state) = memfd_state(file) else {
        return Ok(());
    };
    if len == 0 {
        return Ok(());
    }
    let end = offset
        .checked_add(len as u64)
        .ok_or(AxError::from(LinuxError::EFBIG))?;
    if state.seals() & F_SEAL_GROW != 0 && end > file.inner().metadata()?.size {
        return Err(AxError::from(LinuxError::EPERM));
    }
    Ok(())
}

pub fn check_memfd_resize(file: &File, len: u64) -> AxResult {
    let Some(state) = memfd_state(file) else {
        return Ok(());
    };
    let old_len = file.inner().metadata()?.size;
    let seals = state.seals();
    if len < old_len && seals & F_SEAL_SHRINK != 0 {
        return Err(AxError::from(LinuxError::EPERM));
    }
    if len > old_len && seals & F_SEAL_GROW != 0 {
        return Err(AxError::from(LinuxError::EPERM));
    }
    Ok(())
}

fn validate_flags(flags: u32) -> AxResult {
    let huge_bits = MFD_HUGE_MASK << MFD_HUGE_SHIFT;
    let valid = MFD_CLOEXEC | MFD_ALLOW_SEALING | MFD_HUGETLB | huge_bits;

    if flags & !valid != 0 {
        return Err(AxError::InvalidInput);
    }
    if flags & huge_bits != 0 && flags & MFD_HUGETLB == 0 {
        return Err(AxError::InvalidInput);
    }
    Ok(())
}

pub fn sys_memfd_create(name: UserConstPtr<c_char>, flags: u32) -> AxResult<isize> {
    validate_flags(flags)?;

    let raw_name = name.get_as_slice(250)?;
    let name_len = raw_name
        .iter()
        .position(|ch| *ch == 0)
        .ok_or(AxError::InvalidInput)?;
    let mut name_bytes = Vec::with_capacity(name_len);
    for ch in &raw_name[..name_len] {
        name_bytes.push(*ch as u8);
    }
    let name = str::from_utf8(&name_bytes).map_err(|_| AxError::IllegalBytes)?;

    for id in 0..0xffff {
        let path = format!("/tmp/memfd-{id:04x}-{name}");
        let fs = FS_CONTEXT.lock().clone();
        if fs.resolve(&path).is_err() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&fs, &path)?
                .into_file()?;
            file.location()
                .user_data()
                .insert(MemfdState::new(flags & MFD_ALLOW_SEALING != 0));
            let cloexec = flags & MFD_CLOEXEC != 0;
            return File::new(file).add_to_fd_table(cloexec).map(|fd| fd as _);
        }
    }
    Err(AxError::TooManyOpenFiles)
}
