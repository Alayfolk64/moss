use alloc::{borrow::Cow, format, sync::Arc};
use core::{ffi::c_int, mem::size_of, net::Ipv4Addr, ops::Deref, task::Context};

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
const SIOCGIFCONF: u32 = 0x8912;
const SIOCGIFFLAGS: u32 = 0x8913;
const SIOCGIFADDR: u32 = 0x8915;
const SIOCGIFDSTADDR: u32 = 0x8917;
const SIOCGIFBRDADDR: u32 = 0x8919;
const SIOCGIFNETMASK: u32 = 0x891b;
const SIOCGIFMETRIC: u32 = 0x891d;
const SIOCGIFMTU: u32 = 0x8921;
const SIOCGIFHWADDR: u32 = 0x8927;
const SIOCGIFINDEX: u32 = 0x8933;
const SIOCGIFTXQLEN: u32 = 0x8942;
const SIOCGIFMAP: u32 = 0x8970;

const AF_INET: u16 = 2;
const ARPHRD_ETHER: u16 = 1;
const ARPHRD_LOOPBACK: u16 = 772;

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

/// Linux's 64-bit `struct ifconf`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InterfaceConfig {
    len: i32,
    _padding: u32,
    requests: usize,
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

    fn set_sockaddr_ipv4(&mut self, address: Ipv4Addr) {
        self.data.fill(0);
        self.data[..2].copy_from_slice(&AF_INET.to_ne_bytes());
        self.data[4..8].copy_from_slice(&address.octets());
    }

    fn set_hardware_address(&mut self, hardware_type: u16, address: &[u8]) {
        self.data.fill(0);
        self.data[..2].copy_from_slice(&hardware_type.to_ne_bytes());
        let len = address.len().min(14);
        self.data[2..2 + len].copy_from_slice(&address[..len]);
    }
}

fn interface_index(name: &str) -> AxResult<i32> {
    match name {
        "lo" => Ok(1),
        "eth0" if axnet::eth0_exists() => Ok(2),
        _ => Err(AxError::NoSuchDevice),
    }
}

fn interface_request(name: &str) -> InterfaceRequest {
    let mut request = InterfaceRequest {
        name: [0; IFNAMSIZ],
        data: [0; 24],
    };
    request.set_name(name);
    request
}

fn interface_ipv4(name: &str) -> AxResult<Ipv4Addr> {
    match name {
        "lo" => Ok(Ipv4Addr::LOCALHOST),
        "eth0" => axnet::ipv4_config()
            .map(|config| config.address)
            .ok_or(AxError::NoSuchDevice),
        _ => Err(AxError::NoSuchDevice),
    }
}

fn prefix_mask(prefix_len: u8) -> Ipv4Addr {
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };
    Ipv4Addr::from(mask)
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
            SIOCGIFNAME
                | SIOCGIFCONF
                | SIOCGIFFLAGS
                | SIOCGIFADDR
                | SIOCGIFDSTADDR
                | SIOCGIFBRDADDR
                | SIOCGIFNETMASK
                | SIOCGIFMETRIC
                | SIOCGIFMTU
                | SIOCGIFHWADDR
                | SIOCGIFINDEX
                | SIOCGIFTXQLEN
                | SIOCGIFMAP
        ) {
            return Err(AxError::NotATty);
        }

        if cmd == SIOCGIFCONF {
            let mut config = (arg as *const InterfaceConfig).vm_read()?;
            let mut names = [Some("lo"), None];
            if axnet::eth0_exists() {
                names[1] = Some("eth0");
            }
            let available = names.iter().flatten().count();
            if config.requests == 0 {
                config.len = (available * size_of::<InterfaceRequest>()) as i32;
                (arg as *mut InterfaceConfig).vm_write(config)?;
                return Ok(0);
            }
            let capacity = config.len.max(0) as usize / size_of::<InterfaceRequest>();
            let mut written = 0;
            for name in names.into_iter().flatten().take(capacity) {
                let mut request = interface_request(name);
                request.set_sockaddr_ipv4(interface_ipv4(name)?);
                unsafe {
                    (config.requests as *mut InterfaceRequest)
                        .add(written)
                        .vm_write(request)?;
                }
                written += 1;
            }
            config.len = (written * size_of::<InterfaceRequest>()) as i32;
            (arg as *mut InterfaceConfig).vm_write(config)?;
            return Ok(0);
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
            SIOCGIFADDR => {
                request.set_sockaddr_ipv4(interface_ipv4(request.interface_name()?)?);
            }
            SIOCGIFDSTADDR => {
                interface_index(request.interface_name()?)?;
                request.set_sockaddr_ipv4(Ipv4Addr::UNSPECIFIED);
            }
            SIOCGIFBRDADDR => {
                let address = match request.interface_name()? {
                    "lo" => Ipv4Addr::new(127, 255, 255, 255),
                    "eth0" => {
                        let config = axnet::ipv4_config().ok_or(AxError::NoSuchDevice)?;
                        let address = u32::from(config.address);
                        let mask = u32::from(prefix_mask(config.prefix_len));
                        Ipv4Addr::from(address | !mask)
                    }
                    _ => return Err(AxError::NoSuchDevice),
                };
                request.set_sockaddr_ipv4(address);
            }
            SIOCGIFNETMASK => {
                let prefix_len = match request.interface_name()? {
                    "lo" => 8,
                    "eth0" => {
                        axnet::ipv4_config()
                            .ok_or(AxError::NoSuchDevice)?
                            .prefix_len
                    }
                    _ => return Err(AxError::NoSuchDevice),
                };
                request.set_sockaddr_ipv4(prefix_mask(prefix_len));
            }
            SIOCGIFHWADDR => match request.interface_name()? {
                "lo" => request.set_hardware_address(ARPHRD_LOOPBACK, &[0; 6]),
                "eth0" => request.set_hardware_address(
                    ARPHRD_ETHER,
                    &axnet::eth0_mac_address().ok_or(AxError::NoSuchDevice)?,
                ),
                _ => return Err(AxError::NoSuchDevice),
            },
            SIOCGIFMETRIC => {
                interface_index(request.interface_name()?)?;
                request.set_i32(0);
            }
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
            SIOCGIFMAP => {
                interface_index(request.interface_name()?)?;
                request.data.fill(0);
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
