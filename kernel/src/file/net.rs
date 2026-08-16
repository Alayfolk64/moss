use alloc::{borrow::Cow, format, sync::Arc};
use core::{ffi::c_int, ops::Deref, task::Context};

use axerrno::{AxError, AxResult};
use axnet::{
    RecvOptions, SendOptions, Socket as SocketInner, SocketOps,
    options::{Configurable, GetSocketOption, SetSocketOption},
};
use axpoll::{IoEvents, Pollable};
use linux_raw_sys::general::{O_PATH, S_IFSOCK};
use starry_vm::{VmMutPtr, VmPtr};

use super::{FileLike, Kstat};
use crate::file::{IoDst, IoSrc, get_file_like};

pub struct Socket(pub SocketInner);

const IFNAMSIZ: usize = 16;
const SIOCGIFNAME: u32 = 0x8910;
const SIOCGIFFLAGS: u32 = 0x8913;
const SIOCGIFMTU: u32 = 0x8921;
const SIOCGIFINDEX: u32 = 0x8933;
const SIOCGIFTXQLEN: u32 = 0x8942;

const IFF_UP: i16 = 0x1;
const IFF_BROADCAST: i16 = 0x2;
const IFF_LOOPBACK: i16 = 0x8;
const IFF_RUNNING: i16 = 0x40;
const IFF_MULTICAST: i16 = 0x1000;

/// Linux's 64-bit `struct ifreq`: a 16-byte name followed by its largest
/// 24-byte union member (`struct ifmap`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InterfaceRequest {
    name: [u8; IFNAMSIZ],
    data: [u8; 24],
}

impl InterfaceRequest {
    fn interface_name(&self) -> AxResult<&str> {
        let len = self
            .name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(IFNAMSIZ);
        core::str::from_utf8(&self.name[..len]).map_err(|_| AxError::InvalidInput)
    }

    fn set_name(&mut self, name: &str) {
        self.name.fill(0);
        let len = name.len().min(IFNAMSIZ - 1);
        self.name[..len].copy_from_slice(&name.as_bytes()[..len]);
    }

    fn value_i32(&self) -> i32 {
        i32::from_ne_bytes(self.data[..4].try_into().unwrap())
    }

    fn set_i32(&mut self, value: i32) {
        self.data[..4].copy_from_slice(&value.to_ne_bytes());
    }

    fn set_i16(&mut self, value: i16) {
        self.data[..2].copy_from_slice(&value.to_ne_bytes());
    }
}

fn interface_index(name: &str) -> AxResult<i32> {
    match name {
        "lo" => Ok(1),
        "eth0" => Ok(2),
        _ => Err(AxError::NoSuchDevice),
    }
}

impl Deref for Socket {
    type Target = SocketInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FileLike for Socket {
    fn read(&self, dst: &mut IoDst) -> AxResult<usize> {
        self.recv(dst, RecvOptions::default())
    }

    fn write(&self, src: &mut IoSrc) -> AxResult<usize> {
        self.send(src, SendOptions::default())
    }

    fn stat(&self) -> AxResult<Kstat> {
        // TODO(mivik): implement stat for sockets
        Ok(Kstat {
            mode: S_IFSOCK | 0o777u32, // rwxrwxrwx
            blksize: 4096,
            ..Default::default()
        })
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> AxResult<usize> {
        if !matches!(
            cmd,
            SIOCGIFNAME | SIOCGIFFLAGS | SIOCGIFMTU | SIOCGIFINDEX | SIOCGIFTXQLEN
        ) {
            return Err(AxError::NotATty);
        }

        let mut request = (arg as *const InterfaceRequest).vm_read()?;
        match cmd {
            SIOCGIFNAME => {
                let name = match request.value_i32() {
                    1 => "lo",
                    2 => "eth0",
                    _ => return Err(AxError::NoSuchDevice),
                };
                request.set_name(name);
            }
            SIOCGIFINDEX => request.set_i32(interface_index(request.interface_name()?)?),
            SIOCGIFTXQLEN => {
                interface_index(request.interface_name()?)?;
                request.set_i32(1000);
            }
            SIOCGIFMTU => {
                let mtu = if request.interface_name()? == "lo" {
                    65_536
                } else {
                    interface_index(request.interface_name()?)?;
                    1500
                };
                request.set_i32(mtu);
            }
            SIOCGIFFLAGS => {
                let flags = match request.interface_name()? {
                    "lo" => IFF_UP | IFF_LOOPBACK | IFF_RUNNING,
                    "eth0" => IFF_UP | IFF_BROADCAST | IFF_RUNNING | IFF_MULTICAST,
                    _ => return Err(AxError::NoSuchDevice),
                };
                request.set_i16(flags);
            }
            _ => unreachable!(),
        }
        (arg as *mut InterfaceRequest).vm_write(request)?;
        Ok(0)
    }

    fn nonblocking(&self) -> bool {
        let mut result = false;
        self.get_option(GetSocketOption::NonBlocking(&mut result))
            .unwrap();
        result
    }

    fn set_nonblocking(&self, nonblocking: bool) -> AxResult<()> {
        self.0
            .set_option(SetSocketOption::NonBlocking(&nonblocking))
    }

    fn path(&self) -> Cow<'_, str> {
        format!("socket:[{}]", self as *const _ as usize).into()
    }

    fn from_fd(fd: c_int) -> AxResult<Arc<Self>>
    where
        Self: Sized + 'static,
    {
        let file_like = get_file_like(fd)?;
        if file_like.status_flags() & O_PATH != 0 {
            return Err(AxError::BadFileDescriptor);
        }
        file_like.downcast_arc().map_err(|_| AxError::NotASocket)
    }
}
impl Pollable for Socket {
    fn poll(&self) -> IoEvents {
        self.0.poll()
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        self.0.register(context, events);
    }
}
