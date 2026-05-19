use alloc::vec::Vec;
use core::ffi::c_char;

use axerrno::{AxError, AxResult, LinuxError};
use axfs::{Ext4Inode, FS_CONTEXT};
use axfs_ng_vfs::Location;
use starry_vm::{vm_load, vm_write_slice};

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
    FS_CONTEXT.lock().resolve(path).map_err(Into::into)
}

fn xattr_exists(loc: &Location, name: &[u8]) -> AxResult<bool> {
    match ext4_inode(loc)?.get_xattr(name, &mut []) {
        Ok(_) => Ok(true),
        Err(err) if LinuxError::from(err) == LinuxError::ENODATA => Ok(false),
        Err(err) => Err(err),
    }
}

fn ext4_inode(loc: &Location) -> AxResult<alloc::sync::Arc<Ext4Inode>> {
    loc.entry().downcast::<Ext4Inode>().map_err(Into::into)
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
    let exists = xattr_exists(loc, &name)?;
    if flags & XATTR_CREATE != 0 && exists {
        return Err(AxError::AlreadyExists);
    }
    if flags & XATTR_REPLACE != 0 && !exists {
        return Err(AxError::from(LinuxError::ENODATA));
    }

    let value = if size == 0 {
        Vec::new()
    } else {
        vm_load(value, size)?
    };
    ext4_inode(loc)?.set_xattr(&name, &value)?;
    Ok(0)
}

fn get_xattr(
    loc: &Location,
    name: *const c_char,
    value: *mut u8,
    size: usize,
) -> AxResult<isize> {
    let name = load_xattr_name(name)?;
    let inode = ext4_inode(loc)?;
    let value_len = inode.get_xattr(&name, &mut [])?;
    if size == 0 {
        return Ok(value_len as _);
    }
    if size < value_len {
        return Err(AxError::OutOfRange);
    }

    let mut buf = alloc::vec![0; value_len];
    inode.get_xattr(&name, &mut buf)?;
    vm_write_slice(value, &buf)?;
    Ok(value_len as _)
}

fn list_xattr(loc: &Location, list: *mut u8, size: usize) -> AxResult<isize> {
    let names = ext4_inode(loc)?.list_xattr()?;
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
    ext4_inode(loc)?.remove_xattr(&name)?;
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
