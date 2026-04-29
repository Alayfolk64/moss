#[cfg(feature = "net")]
mod addr;
#[cfg(feature = "net")]
mod cmsg;
#[cfg(feature = "net")]
mod io;
#[cfg(feature = "net")]
mod name;
#[cfg(feature = "net")]
mod opt;
#[cfg(feature = "net")]
mod socket;

#[cfg(not(feature = "net"))]
use axerrno::{AxError, AxResult, LinuxError};
#[cfg(not(feature = "net"))]
use linux_raw_sys::net::{msghdr, sockaddr, socklen_t};

#[cfg(feature = "net")]
pub use self::{cmsg::*, io::*, name::*, opt::*, socket::*};
#[cfg(not(feature = "net"))]
use crate::mm::{UserConstPtr, UserPtr};

#[cfg(not(feature = "net"))]
#[inline]
fn net_disabled() -> AxError {
    AxError::from(LinuxError::ENOSYS)
}

#[cfg(not(feature = "net"))]
pub fn sys_socket(domain: u32, raw_ty: u32, proto: u32) -> AxResult<isize> {
    let _ = (domain, raw_ty, proto);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_socketpair(
    domain: u32,
    raw_ty: u32,
    proto: u32,
    fds: UserPtr<[i32; 2]>,
) -> AxResult<isize> {
    let _ = (domain, raw_ty, proto, fds);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_bind(fd: i32, addr: UserConstPtr<sockaddr>, addrlen: u32) -> AxResult<isize> {
    let _ = (fd, addr, addrlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_connect(fd: i32, addr: UserConstPtr<sockaddr>, addrlen: u32) -> AxResult<isize> {
    let _ = (fd, addr, addrlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_getsockname(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let _ = (fd, addr, addrlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_getpeername(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let _ = (fd, addr, addrlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_listen(fd: i32, backlog: i32) -> AxResult<isize> {
    let _ = (fd, backlog);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_accept(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let _ = (fd, addr, addrlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_accept4(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
    flags: u32,
) -> AxResult<isize> {
    let _ = (fd, addr, addrlen, flags);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_shutdown(fd: i32, how: u32) -> AxResult<isize> {
    let _ = (fd, how);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_sendto(
    fd: i32,
    buf: *const u8,
    len: usize,
    flags: u32,
    addr: UserConstPtr<sockaddr>,
    addrlen: socklen_t,
) -> AxResult<isize> {
    let _ = (fd, buf, len, flags, addr, addrlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_recvfrom(
    fd: i32,
    buf: *mut u8,
    len: usize,
    flags: u32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let _ = (fd, buf, len, flags, addr, addrlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_sendmsg(fd: i32, msg: UserConstPtr<msghdr>, flags: u32) -> AxResult<isize> {
    let _ = (fd, msg, flags);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_recvmsg(fd: i32, msg: UserPtr<msghdr>, flags: u32) -> AxResult<isize> {
    let _ = (fd, msg, flags);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_getsockopt(
    fd: i32,
    level: u32,
    optname: u32,
    optval: UserPtr<u8>,
    optlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let _ = (fd, level, optname, optval, optlen);
    Err(net_disabled())
}

#[cfg(not(feature = "net"))]
pub fn sys_setsockopt(
    fd: i32,
    level: u32,
    optname: u32,
    optval: UserConstPtr<u8>,
    optlen: socklen_t,
) -> AxResult<isize> {
    let _ = (fd, level, optname, optval, optlen);
    Err(net_disabled())
}
