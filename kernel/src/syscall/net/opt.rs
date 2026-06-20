use axerrno::{AxError, AxResult, LinuxError};
use axnet::options::{Configurable, GetSocketOption, SetSocketOption, TcpInfo};
use linux_raw_sys::net::{SO_ATTACH_BPF, SO_OOBINLINE, SOL_SOCKET, socklen_t};

use crate::{
    file::{BpfProgram, FileLike, Socket},
    mm::{UserConstPtr, UserPtr},
};

const PROTO_TCP: u32 = linux_raw_sys::net::IPPROTO_TCP as u32;

const PROTO_IP: u32 = linux_raw_sys::net::IPPROTO_IP as u32;
const PROTO_IPV6: u32 = linux_raw_sys::net::IPPROTO_IPV6 as u32;
const PROTO_UDP: u32 = linux_raw_sys::net::IPPROTO_UDP as u32;
const MCAST_JOIN_GROUP: u32 = linux_raw_sys::net::MCAST_JOIN_GROUP;
const MCAST_LEAVE_GROUP: u32 = linux_raw_sys::net::MCAST_LEAVE_GROUP;

mod conv {
    use axerrno::{AxError, AxResult};
    use axnet::options::{Linger, UnixCredentials};
    use linux_raw_sys::{
        general::timeval,
        net::{linger, ucred},
    };

    use crate::time::TimeValueLike;

    pub struct Int<T>(T);

    impl<T: TryFrom<i32> + TryInto<i32>> Int<T> {
        pub fn sys_to_rust(val: i32) -> AxResult<T> {
            T::try_from(val).map_err(|_| AxError::InvalidInput)
        }

        pub fn rust_to_sys(val: T) -> AxResult<i32> {
            val.try_into().map_err(|_| AxError::InvalidInput)
        }
    }

    pub struct IntBool;

    impl IntBool {
        pub fn sys_to_rust(val: i32) -> AxResult<bool> {
            Ok(val != 0)
        }

        pub fn rust_to_sys(val: bool) -> AxResult<i32> {
            Ok(val as _)
        }
    }

    pub struct Duration;

    impl Duration {
        pub fn sys_to_rust(val: timeval) -> AxResult<core::time::Duration> {
            val.try_into_time_value()
        }

        pub fn rust_to_sys(val: core::time::Duration) -> AxResult<timeval> {
            Ok(timeval::from_time_value(val))
        }
    }

    pub struct Ucred;

    impl Ucred {
        pub fn sys_to_rust(val: ucred) -> AxResult<UnixCredentials> {
            Ok(UnixCredentials {
                pid: val.pid,
                uid: val.uid,
                gid: val.gid,
            })
        }

        pub fn rust_to_sys(val: UnixCredentials) -> AxResult<ucred> {
            Ok(ucred {
                pid: val.pid,
                uid: val.uid,
                gid: val.gid,
            })
        }
    }

    pub struct LingerConv;

    impl LingerConv {
        pub fn sys_to_rust(val: linger) -> AxResult<Linger> {
            Ok(Linger {
                enabled: val.l_onoff != 0,
                timeout_secs: val.l_linger,
            })
        }

        pub fn rust_to_sys(val: Linger) -> AxResult<linger> {
            Ok(linger {
                l_onoff: val.enabled as _,
                l_linger: val.timeout_secs,
            })
        }
    }
}

macro_rules! call_dispatch {
    ($dispatch:ident, $pat:expr) => {{
        use conv::*;
        use linux_raw_sys::net::*;

        call_dispatch! {
            $dispatch, $pat,
            (SOL_SOCKET, SO_REUSEADDR) => ReuseAddress as IntBool,
            (SOL_SOCKET, SO_ERROR) => Error,
            (SOL_SOCKET, SO_DONTROUTE) => DontRoute as IntBool,
            (SOL_SOCKET, SO_SNDBUF) => SendBuffer as Int<usize>,
            (SOL_SOCKET, SO_RCVBUF) => ReceiveBuffer as Int<usize>,
            (SOL_SOCKET, SO_SNDBUFFORCE) => SendBufferForce as Int<usize>,
            (SOL_SOCKET, SO_RCVBUFFORCE) => ReceiveBufferForce as Int<usize>,
            (SOL_SOCKET, SO_KEEPALIVE) => KeepAlive as IntBool,
            (SOL_SOCKET, SO_LINGER) => Linger as LingerConv,
            (SOL_SOCKET, SO_RCVTIMEO) => ReceiveTimeout as Duration,
            (SOL_SOCKET, SO_SNDTIMEO) => SendTimeout as Duration,
            (SOL_SOCKET, SO_PASSCRED) => PassCredentials as IntBool,
            (SOL_SOCKET, SO_PEERCRED) => PeerCredentials as Ucred,

            (PROTO_TCP, TCP_NODELAY) => NoDelay as IntBool,
            (PROTO_TCP, TCP_QUICKACK) => QuickAck as IntBool,
            (PROTO_TCP, TCP_MAXSEG) => MaxSegment as Int<usize>,

            (PROTO_IP, IP_TTL) => Ttl as Int<u8>,
            (PROTO_IP, IP_RECVERR) => ReceiveError as IntBool,
            (PROTO_IPV6, IPV6_V6ONLY) => Ipv6Only as IntBool,
            (PROTO_IPV6, IPV6_RECVERR) => ReceiveError as IntBool,
        }
    }};
    ($dispatch:ident, $in:expr, $($pat:pat => $which:ident $(as $conv:ty)?),* $(,)?) => {
        match $in {
            $(
                $pat => {
                    dispatch!($which $(as $conv)?);
                }
            )*
            _ => return Err(AxError::from(LinuxError::ENOPROTOOPT)),
        }
    }
}

pub fn sys_getsockopt(
    fd: i32,
    level: u32,
    optname: u32,
    optval: UserPtr<u8>,
    optlen: UserPtr<socklen_t>,
) -> AxResult<isize> {
    let optlen = optlen.get_as_mut()?;
    if *optlen > i32::MAX as socklen_t {
        return Err(AxError::InvalidInput);
    }
    debug!(
        "sys_getsockopt <= fd: {}, level: {}, optname: {}, optval: {:?}, optlen: {}",
        fd,
        level,
        optname,
        optval.address(),
        optlen,
    );

    fn get<'a, T: 'static>(val: UserPtr<u8>, len: &mut socklen_t) -> AxResult<&'a mut T> {
        if (*len as usize) < size_of::<T>() {
            return Err(AxError::InvalidInput);
        }
        *len = size_of::<T>() as socklen_t;
        val.cast().get_as_mut()
    }

    let socket = Socket::from_fd(fd)?;
    if unsupported_getsockopt(level, optname) {
        check_getsockopt_output(optval, *optlen)?;
        return Err(AxError::from(LinuxError::EOPNOTSUPP));
    }

    if level == PROTO_TCP && optname == linux_raw_sys::net::TCP_INFO {
        if *optlen == 0 {
            return Err(AxError::InvalidInput);
        }
        let mut info = TcpInfo::default();
        socket.get_option(GetSocketOption::TcpInfo(&mut info))?;
        let len = (*optlen as usize).min(size_of::<TcpInfo>());
        let info_bytes =
            unsafe { core::slice::from_raw_parts(&info as *const _ as *const u8, len) };
        optval.get_as_mut_slice(len)?.copy_from_slice(info_bytes);
        *optlen = len as socklen_t;
        return Ok(0);
    }

    macro_rules! dispatch {
        ($which:ident) => {
            socket.get_option(GetSocketOption::$which(get(optval, optlen)?))?;
        };
        ($which:ident as $conv:ty) => {
            let mut val = Default::default();
            socket.get_option(GetSocketOption::$which(&mut val))?;
            *get(optval, optlen)? = <$conv>::rust_to_sys(val)?;
        };
    }
    call_dispatch!(dispatch, (level, optname));

    Ok(0)
}

fn unsupported_getsockopt(level: u32, optname: u32) -> bool {
    !matches!(level, SOL_SOCKET | PROTO_TCP | PROTO_IP | PROTO_IPV6)
        || level == PROTO_UDP
        || (level == SOL_SOCKET && optname == SO_OOBINLINE)
}

fn check_getsockopt_output(optval: UserPtr<u8>, optlen: socklen_t) -> AxResult<()> {
    if (optlen as usize) < size_of::<i32>() {
        return Err(AxError::InvalidInput);
    }
    optval.get_as_mut_slice(size_of::<i32>()).map(|_| ())
}

pub fn sys_setsockopt(
    fd: i32,
    level: u32,
    optname: u32,
    optval: UserConstPtr<u8>,
    optlen: socklen_t,
) -> AxResult<isize> {
    debug!(
        "sys_setsockopt <= fd: {}, level: {}, optname: {}, optval: {:?}, optlen: {}",
        fd,
        level,
        optname,
        optval.address(),
        optlen
    );

    fn get<'a, T: 'static>(val: UserConstPtr<u8>, len: socklen_t) -> AxResult<&'a T> {
        if len as usize != size_of::<T>() {
            return Err(AxError::InvalidInput);
        }
        val.cast().get_as_ref()
    }

    let socket = Socket::from_fd(fd)?;
    if level == SOL_SOCKET && optname == SO_ATTACH_BPF {
        let prog_fd = *get::<i32>(optval, optlen)?;
        BpfProgram::from_fd(prog_fd)?.execute()?;
        return Ok(0);
    }

    if level == PROTO_IP {
        if optname == MCAST_JOIN_GROUP {
            if (optlen as usize) < size_of::<linux_raw_sys::net::group_req>() {
                return Err(AxError::InvalidInput);
            }
            let _group_req = optval
                .cast::<linux_raw_sys::net::group_req>()
                .get_as_ref()?;
            let join = true;
            socket.set_option(SetSocketOption::McastGroupOp(&join))?;
            return Ok(0);
        }

        if optname == MCAST_LEAVE_GROUP {
            if (optlen as usize) < size_of::<linux_raw_sys::net::group_req>() {
                return Err(AxError::InvalidInput);
            }
            let _group_req = optval
                .cast::<linux_raw_sys::net::group_req>()
                .get_as_ref()?;
            let join = false;
            socket.set_option(SetSocketOption::McastGroupOp(&join))?;
            return Ok(0);
        }
    }

    macro_rules! dispatch {
        ($which:ident) => {
            socket.set_option(SetSocketOption::$which(get(optval, optlen)?))?;
        };
        ($which:ident as $conv:ty) => {
            let mut val = <$conv>::sys_to_rust(*get(optval, optlen)?)?;
            socket.set_option(SetSocketOption::$which(&mut val))?;
        };
    }
    call_dispatch!(dispatch, (level, optname));

    Ok(0)
}
