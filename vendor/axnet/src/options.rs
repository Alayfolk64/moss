use core::time::Duration;

use axerrno::{AxError, AxResult, LinuxError};
use enum_dispatch::enum_dispatch;

macro_rules! define_options {
    ($($name:ident($value:ty),)*) => {
        /// Operation to get a socket option.
        ///
        /// See [`Configurable::get_option`].
        pub enum GetSocketOption<'a> {
            $(
                $name(&'a mut $value),
            )*
        }

        /// Operation to set a socket option.
        ///
        /// See [`Configurable::set_option`].
        #[derive(Clone, Copy)]
        pub enum SetSocketOption<'a> {
            $(
                $name(&'a $value),
            )*
        }
    };
}

/// Corresponds to `struct ucred` in Linux.
#[repr(C)]
#[derive(Default, Debug, Clone)]
pub struct UnixCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}
impl UnixCredentials {
    pub fn new(pid: u32) -> Self {
        UnixCredentials {
            pid,
            uid: 0,
            gid: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TcpInfo {
    pub tcpi_state: u8,
    pub tcpi_ca_state: u8,
    pub tcpi_retransmits: u8,
    pub tcpi_probes: u8,
    pub tcpi_backoff: u8,
    pub tcpi_options: u8,
    pub tcpi_bitfields: [u8; 2],
    pub tcpi_rto: u32,
    pub tcpi_ato: u32,
    pub tcpi_snd_mss: u32,
    pub tcpi_rcv_mss: u32,
    pub tcpi_unacked: u32,
    pub tcpi_sacked: u32,
    pub tcpi_lost: u32,
    pub tcpi_retrans: u32,
    pub tcpi_fackets: u32,
    pub tcpi_last_data_sent: u32,
    pub tcpi_last_ack_sent: u32,
    pub tcpi_last_data_recv: u32,
    pub tcpi_last_ack_recv: u32,
    pub tcpi_pmtu: u32,
    pub tcpi_rcv_ssthresh: u32,
    pub tcpi_rtt: u32,
    pub tcpi_rttvar: u32,
    pub tcpi_snd_ssthresh: u32,
    pub tcpi_snd_cwnd: u32,
    pub tcpi_advmss: u32,
    pub tcpi_reordering: u32,
    pub tcpi_rcv_rtt: u32,
    pub tcpi_rcv_space: u32,
    pub tcpi_total_retrans: u32,
}

define_options! {
    // ---- Socket level options (SO_*) ----
    ReuseAddress(bool),
    Error(i32),
    DontRoute(bool),
    SendBuffer(usize),
    ReceiveBuffer(usize),
    KeepAlive(bool),
    SendTimeout(Duration),
    ReceiveTimeout(Duration),
    SendBufferForce(usize),
    PassCredentials(bool),
    PeerCredentials(UnixCredentials),

    // --- TCP level options (TCP_*) ----
    NoDelay(bool),
    MaxSegment(usize),
    TcpInfo(TcpInfo),

    // ---- IP level options (IP_*) ----
    Ttl(u8),
    Ipv6Only(bool),
    // MCAST_JOIN_GROUP/MCAST_LEAVE_GROUP state used by kernel syscall mapping.
    McastGroupOp(bool),

    // ---- Extra options ----
    NonBlocking(bool),
}

/// Trait for configurable socket-like objects.
#[enum_dispatch]
pub trait Configurable {
    /// Get a socket option, returns `true` if the socket supports the option.
    fn get_option_inner(&self, opt: &mut GetSocketOption) -> AxResult<bool>;
    /// Set a socket option, returns `true` if the socket supports the option.
    fn set_option_inner(&self, opt: SetSocketOption) -> AxResult<bool>;

    fn get_option(&self, mut opt: GetSocketOption) -> AxResult {
        self.get_option_inner(&mut opt).and_then(|supported| {
            if !supported {
                Err(AxError::from(LinuxError::ENOPROTOOPT))
            } else {
                Ok(())
            }
        })
    }
    fn set_option(&self, opt: SetSocketOption) -> AxResult {
        self.set_option_inner(opt).and_then(|supported| {
            if !supported {
                Err(AxError::from(LinuxError::ENOPROTOOPT))
            } else {
                Ok(())
            }
        })
    }
}
