use alloc::vec::Vec;
use core::ffi::{c_char, c_int};

use axerrno::{AxError, AxResult, LinuxError};
use axfs::FS_CONTEXT;
use axfs_ng_vfs::Location;
use linux_raw_sys::general::{FS_APPEND_FL, FS_IMMUTABLE_FL};
use starry_vm::{vm_load, vm_write_slice};

use crate::file::{Directory, File, get_file_like, inode_flags};
use crate::mm::vm_load_string;

const XATTR_CREATE: u32 = 1;
const XATTR_REPLACE: u32 = 2;
const XATTR_SIZE_MAX: usize = 65536;
const XATTR_NAME_MAX: usize = 255;

fn load_xattr_name(name: *const c_char) -> AxResult<Vec<u8>> {
    let name = vm_load_string(name)?;
    if name.is_empty() || name.len() > XATTR_NAME_MAX {
        return Err(AxError::OutOfRange);
    }
    Ok(name.into_bytes())
}

fn resolve_path(path: *const c_char) -> AxResult<Location> {
    let path = vm_load_string(path)?;
    if path.is_empty() {
        return Err(AxError::NotFound);
    }
    FS_CONTEXT.lock().resolve(path).map_err(Into::into)
}

fn resolve_path_no_follow(path: *const c_char) -> AxResult<Location> {
    let path = vm_load_string(path)?;
    if path.is_empty() {
        return Err(AxError::NotFound);
    }
    FS_CONTEXT
        .lock()
        .resolve_no_follow(path)
        .map_err(Into::into)
}

fn resolve_fd(fd: c_int) -> AxResult<Location> {
    resolve_fd_optional(fd)?.ok_or(AxError::BadFileDescriptor)
}

fn resolve_fd_optional(fd: c_int) -> AxResult<Option<Location>> {
    let file_like = get_file_like(fd)?;
    if let Some(file) = file_like.downcast_ref::<File>() {
        Ok(Some(file.inner().backend()?.location().clone()))
    } else if let Some(dir) = file_like.downcast_ref::<Directory>() {
        Ok(Some(dir.inner().clone()))
    } else {
        Ok(None)
    }
}

fn xattr_exists(loc: &Location, name: &[u8]) -> AxResult<bool> {
    match loc.get_xattr(name, &mut []) {
        Ok(_) => Ok(true),
        Err(err) if LinuxError::from(err) == LinuxError::ENODATA => Ok(false),
        Err(err) => Err(err),
    }
}

fn check_xattr_writable(loc: &Location) -> AxResult<()> {
    if let Ok(path) = loc.absolute_path() {
        if inode_flags(path.as_str()) & (FS_IMMUTABLE_FL | FS_APPEND_FL) != 0 {
            return Err(AxError::OperationNotPermitted);
        }
    }
    Ok(())
}

fn set_xattr(
    loc: &Location,
    name: *const c_char,
    value: *const u8,
    size: usize,
    flags: u32,
) -> AxResult<isize> {
    if flags & !(XATTR_CREATE | XATTR_REPLACE) != 0
        || flags & (XATTR_CREATE | XATTR_REPLACE) == (XATTR_CREATE | XATTR_REPLACE)
    {
        return Err(AxError::InvalidInput);
    }
    if size > XATTR_SIZE_MAX {
        return Err(AxError::ArgumentListTooLong);
    }
    let name = load_xattr_name(name)?;
    check_xattr_writable(loc)?;
    if flags & XATTR_CREATE != 0 {
        if xattr_exists(loc, &name)? {
            return Err(AxError::AlreadyExists);
        }
    } else if flags & XATTR_REPLACE != 0 && !xattr_exists(loc, &name)? {
        return Err(AxError::from(LinuxError::ENODATA));
    }

    let value = if size == 0 {
        Vec::new()
    } else {
        vm_load(value, size)?
    };
    loc.set_xattr(&name, &value)?;
    Ok(0)
}

fn get_xattr(loc: &Location, name: *const c_char, value: *mut u8, size: usize) -> AxResult<isize> {
    let name = load_xattr_name(name)?;
    let value_len = loc.get_xattr(&name, &mut [])?;
    if size == 0 {
        return Ok(value_len as _);
    }
    if size < value_len {
        return Err(AxError::OutOfRange);
    }

    let mut buf = alloc::vec![0; value_len];
    loc.get_xattr(&name, &mut buf)?;
    vm_write_slice(value, &buf)?;
    Ok(value_len as _)
}

fn list_xattr(loc: &Location, list: *mut u8, size: usize) -> AxResult<isize> {
    let names = loc.list_xattr()?;
    if size == 0 {
        return Ok(names.len() as _);
    }
    if size < names.len() {
        return Err(AxError::OutOfRange);
    }

    vm_write_slice(list, &names)?;
    Ok(names.len() as _)
}

fn remove_xattr(loc: &Location, name: *const c_char) -> AxResult<isize> {
    let name = load_xattr_name(name)?;
    loc.remove_xattr(&name)?;
    Ok(0)
}

pub fn sys_setxattr(
    path: *const c_char,
    name: *const c_char,
    value: *const u8,
    size: usize,
    flags: u32,
) -> AxResult<isize> {
    debug!("sys_setxattr <= path: {path:p}, name: {name:p}, size: {size}, flags: {flags}");
    set_xattr(&resolve_path(path)?, name, value, size, flags)
}

pub fn sys_getxattr(
    path: *const c_char,
    name: *const c_char,
    value: *mut u8,
    size: usize,
) -> AxResult<isize> {
    debug!("sys_getxattr <= path: {path:p}, name: {name:p}, size: {size}");
    get_xattr(&resolve_path(path)?, name, value, size)
}

pub fn sys_listxattr(path: *const c_char, list: *mut u8, size: usize) -> AxResult<isize> {
    debug!("sys_listxattr <= path: {path:p}, size: {size}");
    list_xattr(&resolve_path(path)?, list, size)
}

pub fn sys_removexattr(path: *const c_char, name: *const c_char) -> AxResult<isize> {
    debug!("sys_removexattr <= path: {path:p}, name: {name:p}");
    remove_xattr(&resolve_path(path)?, name)
}

pub fn sys_lsetxattr(
    path: *const c_char,
    name: *const c_char,
    value: *const u8,
    size: usize,
    flags: u32,
) -> AxResult<isize> {
    debug!("sys_lsetxattr <= path: {path:p}, name: {name:p}, size: {size}, flags: {flags}");
    set_xattr(&resolve_path_no_follow(path)?, name, value, size, flags)
}

pub fn sys_lgetxattr(
    path: *const c_char,
    name: *const c_char,
    value: *mut u8,
    size: usize,
) -> AxResult<isize> {
    debug!("sys_lgetxattr <= path: {path:p}, name: {name:p}, size: {size}");
    get_xattr(&resolve_path_no_follow(path)?, name, value, size)
}

pub fn sys_llistxattr(path: *const c_char, list: *mut u8, size: usize) -> AxResult<isize> {
    debug!("sys_llistxattr <= path: {path:p}, size: {size}");
    list_xattr(&resolve_path_no_follow(path)?, list, size)
}

pub fn sys_lremovexattr(path: *const c_char, name: *const c_char) -> AxResult<isize> {
    debug!("sys_lremovexattr <= path: {path:p}, name: {name:p}");
    remove_xattr(&resolve_path_no_follow(path)?, name)
}

pub fn sys_fsetxattr(
    fd: c_int,
    name: *const c_char,
    value: *const u8,
    size: usize,
    flags: u32,
) -> AxResult<isize> {
    debug!("sys_fsetxattr <= fd: {fd}, name: {name:p}, size: {size}, flags: {flags}");
    set_xattr(&resolve_fd(fd)?, name, value, size, flags)
}

pub fn sys_fgetxattr(
    fd: c_int,
    name: *const c_char,
    value: *mut u8,
    size: usize,
) -> AxResult<isize> {
    debug!("sys_fgetxattr <= fd: {fd}, name: {name:p}, size: {size}");
    if let Some(loc) = resolve_fd_optional(fd)? {
        get_xattr(&loc, name, value, size)
    } else {
        load_xattr_name(name)?;
        Err(AxError::from(LinuxError::ENODATA))
    }
}

pub fn sys_flistxattr(fd: c_int, list: *mut u8, size: usize) -> AxResult<isize> {
    debug!("sys_flistxattr <= fd: {fd}, size: {size}");
    list_xattr(&resolve_fd(fd)?, list, size)
}

pub fn sys_fremovexattr(fd: c_int, name: *const c_char) -> AxResult<isize> {
    debug!("sys_fremovexattr <= fd: {fd}, name: {name:p}");
    remove_xattr(&resolve_fd(fd)?, name)
}
