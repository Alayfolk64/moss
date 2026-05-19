use alloc::vec::Vec;
use core::{
    ffi::{c_char, c_void},
    mem, ptr, slice,
};

use crate::{
    Ext4Error, Ext4Result, InodeRef, SystemHal,
    error::Context,
    ffi::{
        EINVAL, ext4_extract_xattr_name, ext4_get_xattr_name_prefix, ext4_xattr_get,
        ext4_xattr_list, ext4_xattr_list_entry, ext4_xattr_remove, ext4_xattr_set,
    },
};

struct XattrName {
    index: u8,
    name: *const c_char,
    len: usize,
}

fn split_xattr_name(full_name: &[u8]) -> Ext4Result<XattrName> {
    let mut index = 0;
    let mut len = 0;
    let mut found = false;
    let name = unsafe {
        ext4_extract_xattr_name(
            full_name.as_ptr() as _,
            full_name.len(),
            &mut index,
            &mut len,
            &mut found,
        )
    };
    if found {
        Ok(XattrName { index, name, len })
    } else {
        Err(Ext4Error::new(EINVAL as _, None))
    }
}

impl<Hal: SystemHal> InodeRef<Hal> {
    pub fn set_xattr(&mut self, name: &[u8], value: &[u8]) -> Ext4Result {
        let name = split_xattr_name(name)?;
        unsafe {
            ext4_xattr_set(
                self.inner.as_mut(),
                name.index,
                name.name,
                name.len,
                value.as_ptr() as *const c_void,
                value.len(),
            )
            .context("ext4_xattr_set")
        }
    }

    pub fn get_xattr(&mut self, name: &[u8], buf: &mut [u8]) -> Ext4Result<usize> {
        let name = split_xattr_name(name)?;
        let mut data_len = 0;
        unsafe {
            ext4_xattr_get(
                self.inner.as_mut(),
                name.index,
                name.name,
                name.len,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                &mut data_len,
            )
            .context("ext4_xattr_get")?;
        }
        Ok(data_len)
    }

    pub fn list_xattr(&mut self) -> Ext4Result<Vec<u8>> {
        let mut list_len = 0;
        unsafe {
            ext4_xattr_list(self.inner.as_mut(), ptr::null_mut(), &mut list_len)
                .context("ext4_xattr_list")?;
        }
        if list_len == 0 {
            return Ok(Vec::new());
        }

        let words = list_len.div_ceil(mem::size_of::<usize>());
        let mut storage = alloc::vec![0usize; words];
        let list = storage.as_mut_ptr() as *mut ext4_xattr_list_entry;
        unsafe {
            ext4_xattr_list(self.inner.as_mut(), list, &mut list_len).context("ext4_xattr_list")?;
        }

        let mut out = Vec::new();
        let mut entry = list;
        while !entry.is_null() {
            let mut prefix_len = 0;
            let prefix =
                unsafe { ext4_get_xattr_name_prefix((*entry).name_index, &mut prefix_len) };
            if prefix.is_null() {
                return Err(Ext4Error::new(EINVAL as _, None));
            }

            unsafe {
                out.extend_from_slice(slice::from_raw_parts(prefix as *const u8, prefix_len));
                out.extend_from_slice(slice::from_raw_parts(
                    (*entry).name as *const u8,
                    (*entry).name_len,
                ));
                out.push(0);
                entry = (*entry).next;
            }
        }
        Ok(out)
    }

    pub fn remove_xattr(&mut self, name: &[u8]) -> Ext4Result {
        let name = split_xattr_name(name)?;
        unsafe {
            ext4_xattr_remove(self.inner.as_mut(), name.index, name.name, name.len)
                .context("ext4_xattr_remove")
        }
    }
}
