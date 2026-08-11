use alloc::sync::Arc;

use axerrno::{AxError, AxResult, LinuxError};
#[cfg(feature = "vsock")]
use axnet::vsock::{VsockSocket, VsockStreamTransport};
use axnet::{
    IpFamily, Shutdown, Socket as SocketInner, SocketAddrEx, SocketOps,
    tcp::TcpSocket,
    udp::UdpSocket,
    unix::{DgramTransport, StreamTransport, UnixSocket},
};
use axtask::current;
use linux_raw_sys::{
    general::{O_CLOEXEC, O_NONBLOCK},
    net::{
        AF_INET, AF_INET6, AF_UNIX, AF_VSOCK, IPPROTO_SCTP, IPPROTO_TCP, IPPROTO_UDP,
        IPPROTO_UDPLITE, SHUT_RD, SHUT_RDWR, SHUT_WR, SOCK_DGRAM, SOCK_SEQPACKET, SOCK_STREAM,
        sockaddr, socklen_t,
    },
};

use super::addr::SocketAddrExt;
use crate::{
    file::{FileLike, Socket, add_file_like},
    mm::{UserConstPtr, UserPtr},
    task::AsThread,
};

fn check_privileged_bind(addr: &SocketAddrEx) -> AxResult {
    if let SocketAddrEx::Ip(addr) = addr {
        let euid = current().as_thread().proc_data.euid();
        if addr.port() != 0 && addr.port() < 1024 && euid != 0 {
            return Err(AxError::from(LinuxError::EACCES));
        }
    }
    Ok(())
}

pub fn sys_socket(domain: u32, raw_ty: u32, proto: u32) -> AxResult<isize> {
    debug!("sys_socket <= domain: {domain}, ty: {raw_ty}, proto: {proto}");
    let ty = raw_ty & 0xFF;
    let ip_family = match domain {
        AF_INET => Some(IpFamily::Ipv4),
        AF_INET6 => Some(IpFamily::Ipv6),
        _ => None,
    };

    let pid = current().as_thread().proc_data.proc.pid();
    let socket = match (domain, ty) {
        (AF_INET | AF_INET6, SOCK_STREAM) => {
            if proto != 0 && proto != IPPROTO_TCP as _ && proto != IPPROTO_SCTP as _ {
                return Err(AxError::from(LinuxError::EPROTONOSUPPORT));
            }
            SocketInner::Tcp(TcpSocket::new(ip_family.unwrap()))
        }
        (AF_INET | AF_INET6, SOCK_DGRAM) => {
            if proto != 0 && proto != IPPROTO_UDP as _ && proto != IPPROTO_UDPLITE as _ {
                return Err(AxError::from(LinuxError::EPROTONOSUPPORT));
            }
            SocketInner::Udp(UdpSocket::new(ip_family.unwrap()))
        }
        (AF_UNIX, SOCK_STREAM) => SocketInner::Unix(UnixSocket::new(StreamTransport::new(pid))),
        (AF_UNIX, SOCK_SEQPACKET) => SocketInner::Unix(UnixSocket::new(StreamTransport::new(pid))),
        (AF_UNIX, SOCK_DGRAM) => SocketInner::Unix(UnixSocket::new(DgramTransport::new(pid))),
        #[cfg(feature = "vsock")]
        (AF_VSOCK, SOCK_STREAM) => {
            SocketInner::Vsock(VsockSocket::new(VsockStreamTransport::new()))
        }
        (AF_INET | AF_INET6, _) | (AF_UNIX, _) | (AF_VSOCK, _) => {
            warn!("Unsupported socket type: domain: {domain}, ty: {ty}");
            return Err(AxError::from(LinuxError::ESOCKTNOSUPPORT));
        }
        _ => {
            return Err(AxError::from(LinuxError::EAFNOSUPPORT));
        }
    };
    let socket = Socket(socket);

    if raw_ty & O_NONBLOCK != 0 {
        socket.set_nonblocking(true)?;
    }
    let cloexec = raw_ty & O_CLOEXEC != 0;

    socket.add_to_fd_table(cloexec).map(|fd| fd as isize)
}

pub fn sys_bind(fd: i32, addr: UserConstPtr<sockaddr>, addrlen: u32) -> AxResult<isize> {
    let addr = SocketAddrEx::read_from_user(addr, addrlen)?;
    debug!("sys_bind <= fd: {fd}, addr: {addr:?}");

    check_privileged_bind(&addr)?;
    Socket::from_fd(fd)?.bind(addr)?;

    Ok(0)
}

pub fn sys_connect(fd: i32, addr: UserConstPtr<sockaddr>, addrlen: u32) -> AxResult<isize> {
    let socket = Socket::from_fd(fd)?;
    let addr = SocketAddrEx::read_from_user(addr, addrlen)?;
    debug!("sys_connect <= fd: {fd}, addr: {addr:?}");

    socket.connect(addr).map_err(|e| {
        if e == AxError::WouldBlock {
            AxError::InProgress
        } else {
            e
        }
    })?;

    Ok(0)
}

pub fn sys_listen(fd: i32, backlog: i32) -> AxResult<isize> {
    debug!("sys_listen <= fd: {fd}, backlog: {backlog}");

    if backlog < 0 && backlog != -1 {
        return Err(AxError::InvalidInput);
    }

    Socket::from_fd(fd)?.listen()?;

    Ok(0)
}

pub fn sys_accept(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    sys_accept4(fd, addr, addrlen, 0)
}

pub fn sys_accept4(
    fd: i32,
    addr: UserPtr<sockaddr>,
    addrlen: UserPtr<socklen_t>,
    flags: u32,
) -> AxResult<isize> {
    debug!("sys_accept <= fd: {fd}, flags: {flags}");

    let cloexec = flags & O_CLOEXEC != 0;

    let listener = Socket::from_fd(fd)?;
    let mut socket = Arc::new(Socket(listener.accept()?));
    // The pinned LoongArch nightly has corrupted atomic fields while moving
    // large task and socket aggregates. The accepted socket is now in its
    // final allocation and is still private, so restore only its freshly
    // constructed cooperative-lock state before the first peer-address query.
    unsafe {
        Arc::get_mut(&mut socket)
            .expect("newly allocated accepted socket must be unique")
            .0
            .reinitialize_accepted_after_move();
    }
    if flags & O_NONBLOCK != 0 {
        socket.set_nonblocking(true)?;
    }

    let remote_addr = if !addr.is_null() {
        Some(socket.peer_addr()?)
    } else {
        None
    };
    let fd = add_file_like(socket, cloexec).map(|fd| fd as isize)?;
    debug!("sys_accept => fd: {fd}, addr: {remote_addr:?}");

    if let Some(remote_addr) = remote_addr {
        remote_addr.write_to_user(addr, addrlen.get_as_mut()?)?;
    }

    Ok(fd)
}

pub fn sys_shutdown(fd: i32, how: u32) -> AxResult<isize> {
    debug!("sys_shutdown <= fd: {fd}, how: {how:?}");

    let socket = Socket::from_fd(fd)?;
    let how = match how {
        SHUT_RD => Shutdown::Read,
        SHUT_WR => Shutdown::Write,
        SHUT_RDWR => Shutdown::Both,
        _ => return Err(AxError::InvalidInput),
    };
    socket.shutdown(how).map(|_| 0)
}

pub fn sys_socketpair(
    domain: u32,
    raw_ty: u32,
    proto: u32,
    fds: UserPtr<[i32; 2]>,
) -> AxResult<isize> {
    debug!("sys_socketpair <= domain: {domain}, ty: {raw_ty}, proto: {proto}");
    let ty = raw_ty & 0xFF;

    if domain != AF_UNIX {
        return Err(AxError::from(LinuxError::EAFNOSUPPORT));
    }

    let pid = current().as_thread().proc_data.proc.pid();
    let (sock1, sock2) = match ty {
        SOCK_STREAM => {
            let (sock1, sock2) = StreamTransport::new_pair(pid);
            (UnixSocket::new(sock1), UnixSocket::new(sock2))
        }
        SOCK_DGRAM | SOCK_SEQPACKET => {
            let (sock1, sock2) = DgramTransport::new_pair(pid);
            (UnixSocket::new(sock1), UnixSocket::new(sock2))
        }
        _ => {
            warn!("Unsupported socketpair type: {ty}");
            return Err(AxError::from(LinuxError::ESOCKTNOSUPPORT));
        }
    };
    // Put both socket aggregates in their final allocations before publishing
    // them through the descriptor table.  The pinned LoongArch toolchain has
    // previously corrupted fresh atomic fields while moving large socket
    // aggregates (see the accepted-socket repair above).  In particular, a
    // moved SOCK_SEQPACKET endpoint has intermittently acquired O_NONBLOCK and
    // made Rust's blocking fork/exec status read fail with EAGAIN.  Always
    // write the requested state at the final address, including the common
    // blocking case, instead of trusting the moved default value.
    let sock1 = Arc::new(Socket(SocketInner::Unix(sock1)));
    let sock2 = Arc::new(Socket(SocketInner::Unix(sock2)));
    let nonblocking = raw_ty & O_NONBLOCK != 0;
    if !nonblocking && (sock1.nonblocking() || sock2.nonblocking()) {
        warn!(
            "MOSS_SOCKETPAIR_NONBLOCK_REPAIR pid={} type={} endpoint_nonblocking=[{}, {}]",
            pid,
            ty,
            sock1.nonblocking(),
            sock2.nonblocking(),
        );
    }
    sock1.set_nonblocking(nonblocking)?;
    sock2.set_nonblocking(nonblocking)?;
    let cloexec = raw_ty & O_CLOEXEC != 0;

    *fds.get_as_mut()? = [
        add_file_like(sock1, cloexec)?,
        add_file_like(sock2, cloexec)?,
    ];
    Ok(0)
}
