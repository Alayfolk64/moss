use alloc::{boxed::Box, vec::Vec};
use core::net::{Ipv4Addr, SocketAddr};

use axerrno::{AxError, AxResult, LinuxError};
use axio::prelude::*;
use axnet::{
    CMsgData, RecvControlMessage, RecvFlags, RecvOptions, SendFlags, SendOptions, SocketAddrEx,
    SocketOps,
};
use linux_raw_sys::net::{
    __kernel_sockaddr_storage, MSG_ERRQUEUE, MSG_OOB, MSG_PEEK, MSG_TRUNC, SCM_RIGHTS, SOL_SOCKET,
    cmsghdr, mmsghdr, msghdr, sockaddr, socklen_t,
};

use super::addr::SocketAddrExt;
use crate::{
    file::{FileLike, Socket, add_file_like},
    mm::{IoVec, IoVectorBuf, UserConstPtr, UserPtr, VmBytes, VmBytesMut},
    syscall::net::{CMsg, CMsgBuilder, cmsg_align},
};

fn send_impl(
    fd: i32,
    mut src: impl Read + IoBuf,
    flags: u32,
    addr: UserConstPtr<sockaddr>,
    addrlen: socklen_t,
    cmsg: Vec<CMsgData>,
) -> AxResult<isize> {
    if flags & MSG_OOB != 0 {
        return Err(AxError::from(LinuxError::EOPNOTSUPP));
    }

    let addr = if addr.is_null() || addrlen == 0 {
        None
    } else {
        Some(SocketAddrEx::read_from_user(addr, addrlen)?)
    };

    debug!("sys_send <= fd: {fd}, flags: {flags}, addr: {addr:?}");

    let socket = Socket::from_fd(fd)?;
    let sent = socket.send(
        &mut src,
        SendOptions {
            to: addr,
            flags: SendFlags::default(),
            cmsg,
        },
    )?;

    Ok(sent as isize)
}

pub fn sys_sendto(
    fd: i32,
    buf: *const u8,
    len: usize,
    flags: u32,
    addr: UserConstPtr<sockaddr>,
    addrlen: socklen_t,
) -> AxResult<isize> {
    send_impl(fd, VmBytes::new(buf, len), flags, addr, addrlen, Vec::new())
}

pub fn sys_sendmsg(fd: i32, msg: UserConstPtr<msghdr>, flags: u32) -> AxResult<isize> {
    let msg = msg.get_as_ref()?;
    if msg.msg_iovlen > 1024 {
        return Err(AxError::from(LinuxError::EMSGSIZE));
    }
    let mut cmsg = Vec::new();
    if !msg.msg_control.is_null() {
        let mut ptr = msg.msg_control as usize;
        let ptr_end = ptr
            .checked_add(msg.msg_controllen)
            .ok_or(AxError::InvalidInput)?;
        while ptr + size_of::<cmsghdr>() <= ptr_end {
            let hdr = UserConstPtr::<cmsghdr>::from(ptr).get_as_ref()?;
            if ptr_end - ptr < hdr.cmsg_len {
                return Err(AxError::InvalidInput);
            }
            cmsg.push(Box::new(CMsg::parse(hdr)?) as CMsgData);
            ptr = ptr
                .checked_add(cmsg_align(hdr.cmsg_len))
                .ok_or(AxError::InvalidInput)?;
        }
    }
    send_impl(
        fd,
        IoVectorBuf::new(msg.msg_iov as *const IoVec, msg.msg_iovlen)?.into_io(),
        flags,
        UserConstPtr::from(msg.msg_name as usize),
        msg.msg_namelen as socklen_t,
        cmsg,
    )
}

pub fn sys_sendmmsg(fd: i32, msgvec: UserPtr<mmsghdr>, vlen: u32, flags: u32) -> AxResult<isize> {
    let messages = msgvec.get_as_mut_slice(vlen as usize)?;
    let mut sent = 0;

    for message in messages {
        let msg_ptr = UserConstPtr::from(&message.msg_hdr as *const msghdr);
        match sys_sendmsg(fd, msg_ptr, flags) {
            Ok(len) => {
                message.msg_len = len as _;
                sent += 1;
            }
            Err(_err) if sent > 0 => return Ok(sent),
            Err(err) => return Err(err),
        }
    }

    Ok(sent)
}

fn is_unspecified_remote_addr(addr: &SocketAddrEx) -> bool {
    match addr {
        SocketAddrEx::Ip(SocketAddr::V4(addr)) => addr.ip().is_unspecified() && addr.port() == 0,
        SocketAddrEx::Ip(SocketAddr::V6(addr)) => addr.ip().is_unspecified() && addr.port() == 0,
        SocketAddrEx::Unix(_) => false,
        SocketAddrEx::Netlink(_) => false,
        #[cfg(feature = "vsock")]
        SocketAddrEx::Vsock(_) => false,
    }
}

fn recv_impl(
    fd: i32,
    mut dst: impl Write + IoBufMut,
    flags: u32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
    cmsg_builder: Option<CMsgBuilder>,
) -> AxResult<isize> {
    debug!("sys_recv <= fd: {fd}, flags: {flags}");

    if flags & MSG_OOB != 0 {
        return Err(AxError::InvalidInput);
    }
    if flags & MSG_ERRQUEUE != 0 {
        return Err(AxError::from(LinuxError::EAGAIN));
    }
    if !addr.is_null() {
        let len = *addrlen.get_as_mut()?;
        if len as usize > size_of::<__kernel_sockaddr_storage>() {
            return Err(AxError::InvalidInput);
        }
    }

    let socket = Socket::from_fd(fd)?;
    let mut recv_flags = RecvFlags::empty();
    if flags & MSG_PEEK != 0 {
        recv_flags |= RecvFlags::PEEK;
    }
    if flags & MSG_TRUNC != 0 {
        recv_flags |= RecvFlags::TRUNCATE;
    }

    let mut cmsg = cmsg_builder.as_ref().map(|_| Vec::new());

    let mut remote_addr = prepare_recv_addr(&socket, addr);
    let recv = socket.recv(
        &mut dst,
        RecvOptions {
            from: remote_addr.as_mut(),
            flags: recv_flags,
            cmsg: cmsg.as_mut(),
        },
    )?;
    finish_recv_addr(&socket, remote_addr, addr, addrlen)?;

    if let (Some(mut builder), Some(cmsg)) = (cmsg_builder, cmsg) {
        for cmsg in cmsg {
            let pushed = match cmsg.downcast::<CMsg>() {
                Ok(cmsg) => match *cmsg {
                    CMsg::Rights { fds } => builder.push(SOL_SOCKET, SCM_RIGHTS, |data| {
                        let mut written = 0;
                        for (f, chunk) in
                            fds.into_iter().zip(data.chunks_exact_mut(size_of::<i32>()))
                        {
                            let fd = add_file_like(f, false)?;
                            chunk.copy_from_slice(&fd.to_ne_bytes());
                            written += size_of::<i32>();
                        }
                        Ok(written)
                    })?,
                },
                Err(cmsg) => match cmsg.downcast::<RecvControlMessage>() {
                    Ok(cmsg) => match *cmsg {
                        RecvControlMessage::Ipv6HopLimit { value, cmsg_type } => builder.push(
                            linux_raw_sys::net::IPPROTO_IPV6 as u32,
                            cmsg_type,
                            |data| {
                                let bytes = value.to_ne_bytes();
                                if data.len() < bytes.len() {
                                    return Err(AxError::InvalidInput);
                                }
                                data[..bytes.len()].copy_from_slice(&bytes);
                                Ok(bytes.len())
                            },
                        )?,
                    },
                    Err(_) => {
                        warn!("received unexpected cmsg");
                        continue;
                    }
                },
            };
            if !pushed {
                break;
            }
        }
    }

    debug!("sys_recv => fd: {fd}, recv: {recv}");
    Ok(recv as isize)
}

pub fn sys_recvfrom(
    fd: i32,
    buf: *mut u8,
    len: usize,
    flags: u32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    recv_impl(fd, VmBytesMut::new(buf, len), flags, addr, addrlen, None)
}

pub fn sys_recvmsg(fd: i32, msg: UserPtr<msghdr>, flags: u32) -> AxResult<isize> {
    let msg = msg.get_as_mut()?;
    if msg.msg_iovlen > 1024 {
        return Err(AxError::from(LinuxError::EMSGSIZE));
    }
    // msg_flags is an output field. Linux overwrites it on every recvmsg;
    // leaving the caller's previous stack contents intact can fabricate
    // MSG_TRUNC/MSG_CTRUNC and make otherwise valid ancillary data unusable.
    msg.msg_flags = 0;
    recv_impl(
        fd,
        IoVectorBuf::new(msg.msg_iov as *mut IoVec, msg.msg_iovlen)?.into_io(),
        flags,
        UserPtr::from(msg.msg_name as usize),
        UserPtr::from(&mut msg.msg_namelen as *mut _ as *mut socklen_t),
        (!msg.msg_control.is_null()).then(|| {
            CMsgBuilder::new(
                UserPtr::from(msg.msg_control as *mut cmsghdr),
                &mut msg.msg_controllen,
            )
        }),
    )
}

#[cfg(target_arch = "loongarch64")]
fn prepare_recv_addr(socket: &Socket, addr: UserPtr<sockaddr>) -> Option<SocketAddrEx> {
    let is_netlink = matches!(&socket.0, axnet::Socket::Netlink(_));
    (!addr.is_null() && !is_netlink).then(|| SocketAddrEx::Ip((Ipv4Addr::UNSPECIFIED, 0).into()))
}

#[cfg(target_arch = "loongarch64")]
fn finish_recv_addr(
    socket: &Socket,
    remote_addr: Option<SocketAddrEx>,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<()> {
    if matches!(&socket.0, axnet::Socket::Netlink(_)) {
        return write_kernel_netlink_addr(addr, addrlen);
    }
    write_recv_addr(remote_addr, addr, addrlen)
}

#[cfg(target_arch = "loongarch64")]
fn write_kernel_netlink_addr(addr: UserPtr<sockaddr>, addrlen: UserPtr<socklen_t>) -> AxResult<()> {
    if addr.is_null() {
        return Ok(());
    }
    // The pinned LoongArch nightly corrupts sockaddr_nl when it is carried
    // through SocketAddrEx.  Write the fixed kernel-origin address directly,
    // using volatile byte stores to avoid the compiler's aggregate-copy bug.
    const NETLINK_ADDR_LEN: usize = 12;
    let addrlen = addrlen.get_as_mut()?;
    let output = addr
        .cast::<u8>()
        .get_as_mut_slice((*addrlen as usize).min(NETLINK_ADDR_LEN))?;
    let family = (linux_raw_sys::net::AF_NETLINK as u16).to_ne_bytes();
    for (index, byte) in output.iter_mut().enumerate() {
        let value = family.get(index).copied().unwrap_or(0);
        unsafe { core::ptr::write_volatile(byte, value) };
    }
    *addrlen = NETLINK_ADDR_LEN as _;
    Ok(())
}

#[cfg(not(target_arch = "loongarch64"))]
fn prepare_recv_addr(_socket: &Socket, addr: UserPtr<sockaddr>) -> Option<SocketAddrEx> {
    (!addr.is_null()).then(|| SocketAddrEx::Ip((Ipv4Addr::UNSPECIFIED, 0).into()))
}

#[cfg(not(target_arch = "loongarch64"))]
fn finish_recv_addr(
    _socket: &Socket,
    remote_addr: Option<SocketAddrEx>,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<()> {
    write_recv_addr(remote_addr, addr, addrlen)
}

fn write_recv_addr(
    remote_addr: Option<SocketAddrEx>,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<()> {
    if let Some(remote_addr) = remote_addr
        && !is_unspecified_remote_addr(&remote_addr)
    {
        remote_addr.write_to_user(addr, addrlen.get_as_mut()?)?;
    }
    Ok(())
}
