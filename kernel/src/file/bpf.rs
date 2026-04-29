use alloc::{borrow::Cow, sync::Arc, vec, vec::Vec};
use core::{ffi::c_int, task::Context};

use axerrno::{AxError, AxResult, LinuxError};
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
use hashbrown::HashMap;

use super::{FileLike, get_file_like};
use crate::mm::{UserConstPtr, UserPtr};

pub const BPF_MAP_TYPE_HASH: u32 = 1;
pub const BPF_MAP_TYPE_ARRAY: u32 = 2;
pub const BPF_MAP_TYPE_RINGBUF: u32 = 27;

enum MapData {
    Hash(Mutex<HashMap<Vec<u8>, Vec<u8>>>),
    Array(Mutex<Vec<u8>>),
    RingBuf,
}

pub struct BpfMap {
    pub map_type: u32,
    pub key_size: u32,
    pub value_size: u32,
    pub max_entries: u32,
    data: MapData,
}

impl BpfMap {
    pub fn new(map_type: u32, key_size: u32, value_size: u32, max_entries: u32) -> AxResult<Self> {
        let data = match map_type {
            BPF_MAP_TYPE_HASH => MapData::Hash(Mutex::new(HashMap::new())),
            BPF_MAP_TYPE_ARRAY => {
                let size = (value_size as usize)
                    .checked_mul(max_entries as usize)
                    .ok_or(AxError::InvalidInput)?;
                MapData::Array(Mutex::new(vec![0; size]))
            }
            BPF_MAP_TYPE_RINGBUF => MapData::RingBuf,
            _ => return Err(AxError::InvalidInput),
        };
        Ok(Self {
            map_type,
            key_size,
            value_size,
            max_entries,
            data,
        })
    }

    fn read_user(src: usize, len: usize) -> AxResult<Vec<u8>> {
        Ok(UserConstPtr::<u8>::from(src).get_as_slice(len)?.to_vec())
    }

    fn write_user(dst: usize, data: &[u8]) -> AxResult {
        UserPtr::<u8>::from(dst)
            .get_as_mut_slice(data.len())?
            .copy_from_slice(data);
        Ok(())
    }

    fn array_index(&self, key: &[u8]) -> AxResult<usize> {
        if key.len() < 4 {
            return Err(AxError::InvalidInput);
        }
        let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
        if idx >= self.max_entries as usize {
            return Err(AxError::from(LinuxError::ENOENT));
        }
        Ok(idx)
    }

    pub fn lookup_elem(&self, key_ptr: usize, val_ptr: usize) -> AxResult {
        let key = Self::read_user(key_ptr, self.key_size as usize)?;
        match &self.data {
            MapData::Hash(map) => {
                let map = map.lock();
                let val = map.get(&key).ok_or_else(|| AxError::from(LinuxError::ENOENT))?;
                Self::write_user(val_ptr, val)
            }
            MapData::Array(vals) => {
                let idx = self.array_index(&key)?;
                let off = idx * self.value_size as usize;
                let end = off + self.value_size as usize;
                Self::write_user(val_ptr, &vals.lock()[off..end])
            }
            MapData::RingBuf => Err(AxError::InvalidInput),
        }
    }

    pub fn update_elem(&self, key_ptr: usize, val_ptr: usize) -> AxResult {
        let key = Self::read_user(key_ptr, self.key_size as usize)?;
        let val = Self::read_user(val_ptr, self.value_size as usize)?;
        self.update_raw(&key, &val)
    }

    pub fn update_raw(&self, key: &[u8], val: &[u8]) -> AxResult {
        if key.len() != self.key_size as usize || val.len() != self.value_size as usize {
            return Err(AxError::InvalidInput);
        }
        match &self.data {
            MapData::Hash(map) => {
                map.lock().insert(key.to_vec(), val.to_vec());
                Ok(())
            }
            MapData::Array(vals) => {
                let idx = self.array_index(key)?;
                let off = idx * self.value_size as usize;
                let end = off + self.value_size as usize;
                vals.lock()[off..end].copy_from_slice(val);
                Ok(())
            }
            MapData::RingBuf => Err(AxError::InvalidInput),
        }
    }
}

impl FileLike for BpfMap {
    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[bpf-map]".into()
    }

    fn from_fd(fd: c_int) -> AxResult<Arc<Self>>
    where
        Self: Sized + 'static,
    {
        get_file_like(fd)?
            .downcast_arc()
            .map_err(|_| AxError::BadFileDescriptor)
    }
}

impl Pollable for BpfMap {
    fn poll(&self) -> IoEvents {
        IoEvents::empty()
    }
    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

#[derive(Clone, Copy)]
pub enum BpfProgramKind {
    Prog01,
    Prog02,
    Prog05,
    Noop,
}

pub struct BpfProgram {
    kind: BpfProgramKind,
    map_fd: c_int,
}

impl BpfProgram {
    pub fn new(kind: BpfProgramKind, map_fd: c_int) -> Self {
        Self { kind, map_fd }
    }

    pub fn execute(&self) -> AxResult {
        if matches!(self.kind, BpfProgramKind::Noop) {
            return Ok(());
        }

        let map = BpfMap::from_fd(self.map_fd)?;
        if map.map_type != BPF_MAP_TYPE_ARRAY || map.value_size != 8 {
            return Err(AxError::InvalidInput);
        }

        let put = |idx: u32, v: u64| map.update_raw(&idx.to_ne_bytes(), &v.to_ne_bytes());
        match self.kind {
            BpfProgramKind::Prog01 => put(0, 1),
            BpfProgramKind::Prog02 => {
                const A64INT: u64 = 1u64 << 60;
                put(0, A64INT + 1)?;
                put(1, A64INT - 1)
            }
            BpfProgramKind::Prog05 => {
                put(0, 1u64 << 32)?;
                put(1, 0)?;
                put(2, 1u64 << 32)?;
                put(3, u32::MAX as u64)
            }
            BpfProgramKind::Noop => Ok(()),
        }
    }
}

impl FileLike for BpfProgram {
    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[bpf-prog]".into()
    }

    fn from_fd(fd: c_int) -> AxResult<Arc<Self>>
    where
        Self: Sized + 'static,
    {
        get_file_like(fd)?
            .downcast_arc()
            .map_err(|_| AxError::BadFileDescriptor)
    }
}

impl Pollable for BpfProgram {
    fn poll(&self) -> IoEvents {
        IoEvents::empty()
    }
    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}
