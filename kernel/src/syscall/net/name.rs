use axerrno::{AxError, AxResult, LinuxError};
use axnet::SocketOps;
use linux_raw_sys::net::{sockaddr, socklen_t};

use super::addr::SocketAddrExt;
use crate::{
    file::{FileLike, Socket},
    mm::UserPtr,
};

pub fn sys_getsockname(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let socket = Socket::from_fd(fd)?;
    let local_addr = socket.local_addr()?;
    let addrlen = checked_addrlen(addrlen)?;
    debug!("sys_getsockname <= fd: {fd}, addr: {local_addr:?}");

    local_addr.write_to_user(addr, addrlen)?;
    Ok(0)
}

pub fn sys_getpeername(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let socket = Socket::from_fd(fd)?;
    let peer_addr = socket.peer_addr()?;
    let addrlen = checked_addrlen(addrlen)?;
    debug!("sys_getpeername <= fd: {fd}, addr: {peer_addr:?}");

    peer_addr.write_to_user(addr, addrlen)?;
    Ok(0)
}

fn checked_addrlen(addrlen: UserPtr<socklen_t>) -> AxResult<&'static mut socklen_t> {
    let addrlen = addrlen.get_as_mut()?;
    if *addrlen > i32::MAX as socklen_t {
        return Err(AxError::from(LinuxError::EINVAL));
    }
    Ok(addrlen)
}
