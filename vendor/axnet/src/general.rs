use core::{
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    task::Waker,
    time::Duration,
};

use axerrno::{AxError, AxResult};
use axpoll::{IoEvents, Pollable};
use axtask::future::{block_on, poll_io, timeout};

use crate::{
    SERVICE,
    consts::MAX_SOCKET_BUF_LEN,
    options::{Configurable, GetSocketOption, Linger, SetSocketOption},
    poll_interfaces,
};

const SOCKET_BUFFER_SCALE: usize = 2;

/// General options for all sockets.
pub(crate) struct GeneralOptions {
    /// Whether the socket is non-blocking.
    nonblock: AtomicBool,
    /// Whether the socket should reuse the address.
    reuse_address: AtomicBool,
    /// Whether an IPv6 socket only accepts IPv6 traffic.
    ipv6_only: AtomicBool,
    /// Whether packets should bypass gateways where possible.
    dont_route: AtomicBool,
    /// Whether asynchronous network errors should be reported.
    recv_error: AtomicBool,
    /// SO_LINGER enabled flag.
    linger_enabled: AtomicBool,
    /// SO_LINGER timeout in seconds.
    linger_timeout_secs: AtomicU32,

    send_timeout_nanos: AtomicU64,
    recv_timeout_nanos: AtomicU64,
    send_buffer_len: AtomicU32,
    recv_buffer_len: AtomicU32,

    device_mask: AtomicU32,
}
impl Default for GeneralOptions {
    fn default() -> Self {
        Self::new()
    }
}
impl GeneralOptions {
    pub fn new() -> Self {
        Self {
            nonblock: AtomicBool::new(false),
            reuse_address: AtomicBool::new(false),
            ipv6_only: AtomicBool::new(false),
            dont_route: AtomicBool::new(false),
            recv_error: AtomicBool::new(false),
            linger_enabled: AtomicBool::new(false),
            linger_timeout_secs: AtomicU32::new(0),

            send_timeout_nanos: AtomicU64::new(0),
            recv_timeout_nanos: AtomicU64::new(0),
            send_buffer_len: AtomicU32::new(0),
            recv_buffer_len: AtomicU32::new(0),

            device_mask: AtomicU32::new(0),
        }
    }

    pub fn nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Relaxed)
    }

    pub fn reuse_address(&self) -> bool {
        self.reuse_address.load(Ordering::Relaxed)
    }

    pub fn ipv6_only(&self) -> bool {
        self.ipv6_only.load(Ordering::Relaxed)
    }

    pub fn dont_route(&self) -> bool {
        self.dont_route.load(Ordering::Relaxed)
    }

    pub fn recv_error(&self) -> bool {
        self.recv_error.load(Ordering::Relaxed)
    }

    pub fn linger(&self) -> Linger {
        Linger {
            enabled: self.linger_enabled.load(Ordering::Relaxed),
            timeout_secs: self.linger_timeout_secs.load(Ordering::Relaxed) as i32,
        }
    }

    pub fn set_linger(&self, linger: Linger) {
        self.linger_enabled.store(linger.enabled, Ordering::Relaxed);
        self.linger_timeout_secs
            .store(linger.timeout_secs.max(0) as u32, Ordering::Relaxed);
    }

    pub fn set_ipv6_only(&self, ipv6_only: bool) {
        self.ipv6_only.store(ipv6_only, Ordering::Relaxed);
    }

    pub fn send_timeout(&self) -> Option<Duration> {
        let nanos = self.send_timeout_nanos.load(Ordering::Relaxed);
        (nanos > 0).then(|| Duration::from_nanos(nanos))
    }

    pub fn recv_timeout(&self) -> Option<Duration> {
        let nanos = self.recv_timeout_nanos.load(Ordering::Relaxed);
        (nanos > 0).then(|| Duration::from_nanos(nanos))
    }

    pub fn send_buffer(&self, default: usize) -> usize {
        let size = self.send_buffer_len.load(Ordering::Relaxed) as usize;
        if size == 0 { default } else { size }
    }

    pub fn recv_buffer(&self, default: usize) -> usize {
        let size = self.recv_buffer_len.load(Ordering::Relaxed) as usize;
        if size == 0 { default } else { size }
    }

    pub fn set_send_buffer(&self, size: usize) {
        self.send_buffer_len
            .store(Self::normalize_socket_buffer(size) as u32, Ordering::Relaxed);
    }

    pub fn set_recv_buffer(&self, size: usize) {
        self.recv_buffer_len
            .store(Self::normalize_socket_buffer(size) as u32, Ordering::Relaxed);
    }

    fn normalize_socket_buffer(size: usize) -> usize {
        size.saturating_mul(SOCKET_BUFFER_SCALE)
            .clamp(1, MAX_SOCKET_BUF_LEN)
    }

    pub fn set_device_mask(&self, mask: u32) {
        self.device_mask.store(mask, Ordering::Release);
    }

    pub fn device_mask(&self) -> u32 {
        self.device_mask.load(Ordering::Acquire)
    }

    pub fn register_waker(&self, waker: &Waker) {
        SERVICE.lock().register_waker(self.device_mask(), waker);
    }

    pub fn send_poller<P: Pollable, F: FnMut() -> AxResult<T>, T>(
        &self,
        pollable: &P,
        f: F,
    ) -> AxResult<T> {
        block_on(timeout(
            self.send_timeout(),
            poll_io(pollable, IoEvents::OUT, self.nonblocking(), f),
        ))?
    }

    pub fn recv_poller<P: Pollable, F: FnMut() -> AxResult<T>, T>(
        &self,
        pollable: &P,
        f: F,
    ) -> AxResult<T> {
        block_on(timeout(
            self.recv_timeout(),
            poll_io(pollable, IoEvents::IN, self.nonblocking(), f),
        ))?
    }

    fn network_poller<P: Pollable, F: FnMut() -> AxResult<T>, T>(
        &self,
        pollable: &P,
        events: IoEvents,
        deadline: Option<Duration>,
        mut f: F,
    ) -> AxResult<T> {
        match f() {
            Ok(value) => Ok(value),
            Err(AxError::WouldBlock) => {
                poll_interfaces();
                block_on(timeout(
                    deadline,
                    poll_io(pollable, events, self.nonblocking(), f),
                ))?
            }
            Err(err) => Err(err),
        }
    }

    pub fn network_send_poller<P: Pollable, F: FnMut() -> AxResult<T>, T>(
        &self,
        pollable: &P,
        f: F,
    ) -> AxResult<T> {
        self.network_poller(pollable, IoEvents::OUT, self.send_timeout(), f)
    }

    pub fn network_recv_poller<P: Pollable, F: FnMut() -> AxResult<T>, T>(
        &self,
        pollable: &P,
        f: F,
    ) -> AxResult<T> {
        self.network_poller(pollable, IoEvents::IN, self.recv_timeout(), f)
    }
}
impl Configurable for GeneralOptions {
    fn get_option_inner(&self, option: &mut GetSocketOption) -> AxResult<bool> {
        use GetSocketOption as O;
        match option {
            O::Error(error) => {
                // TODO(mivik): actual logic
                **error = 0;
            }
            O::NonBlocking(nonblock) => {
                **nonblock = self.nonblocking();
            }
            O::ReuseAddress(reuse) => {
                **reuse = self.reuse_address();
            }
            O::DontRoute(dont_route) => {
                **dont_route = self.dont_route();
            }
            O::SendTimeout(timeout) => {
                **timeout = Duration::from_nanos(self.send_timeout_nanos.load(Ordering::Relaxed));
            }
            O::ReceiveTimeout(timeout) => {
                **timeout = Duration::from_nanos(self.recv_timeout_nanos.load(Ordering::Relaxed));
            }
            O::ReceiveError(recv_error) => {
                **recv_error = self.recv_error();
            }
            O::Linger(linger) => {
                **linger = self.linger();
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn set_option_inner(&self, option: SetSocketOption) -> AxResult<bool> {
        use SetSocketOption as O;

        match option {
            O::NonBlocking(nonblock) => {
                self.nonblock.store(*nonblock, Ordering::Relaxed);
            }
            O::ReuseAddress(reuse) => {
                self.reuse_address.store(*reuse, Ordering::Relaxed);
            }
            O::DontRoute(dont_route) => {
                self.dont_route.store(*dont_route, Ordering::Relaxed);
            }
            O::SendTimeout(timeout) => {
                self.send_timeout_nanos
                    .store(timeout.as_nanos() as u64, Ordering::Relaxed);
            }
            O::ReceiveTimeout(timeout) => {
                self.recv_timeout_nanos
                    .store(timeout.as_nanos() as u64, Ordering::Relaxed);
            }
            O::ReceiveError(recv_error) => {
                self.recv_error.store(*recv_error, Ordering::Relaxed);
            }
            O::SendBuffer(size) | O::SendBufferForce(size) => {
                self.set_send_buffer(*size);
            }
            O::ReceiveBuffer(size) | O::ReceiveBufferForce(size) => {
                self.set_recv_buffer(*size);
            }
            O::Linger(linger) => {
                self.set_linger(*linger);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
