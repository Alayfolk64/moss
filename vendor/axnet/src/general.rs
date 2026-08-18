use core::{
    sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering},
    task::Waker,
    time::Duration,
};

use axerrno::{AxError, AxResult, LinuxError};
use axpoll::{IoEvents, Pollable};
use axtask::future::{block_on, interruptible, poll_io, timeout};

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
    /// Whether datagrams may be sent to a broadcast address.
    broadcast: AtomicBool,
    /// Whether an IPv6 socket only accepts IPv6 traffic.
    ipv6_only: AtomicBool,
    /// Whether packets should bypass gateways where possible.
    dont_route: AtomicBool,
    /// Whether asynchronous network errors should be reported.
    recv_error: AtomicBool,
    /// IPv4 type-of-service byte used by OpenSSH's IPQoS setting.
    ip_tos: AtomicU32,
    /// Pending asynchronous socket error, consumed by SO_ERROR.
    pending_error: AtomicI32,
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
            broadcast: AtomicBool::new(false),
            ipv6_only: AtomicBool::new(false),
            dont_route: AtomicBool::new(false),
            recv_error: AtomicBool::new(false),
            ip_tos: AtomicU32::new(0),
            pending_error: AtomicI32::new(0),
            linger_enabled: AtomicBool::new(false),
            linger_timeout_secs: AtomicU32::new(0),

            send_timeout_nanos: AtomicU64::new(0),
            recv_timeout_nanos: AtomicU64::new(0),
            send_buffer_len: AtomicU32::new(0),
            recv_buffer_len: AtomicU32::new(0),

            device_mask: AtomicU32::new(0),
        }
    }

    /// Initializes default socket options at their final unpublished address.
    ///
    /// # Safety
    ///
    /// `dst` must be valid, aligned, uninitialized storage for one
    /// `GeneralOptions` and must remain private until initialization finishes.
    pub(crate) unsafe fn write_unpublished_in_place(dst: *mut Self) {
        unsafe {
            core::ptr::addr_of_mut!((*dst).nonblock).write(AtomicBool::new(false));
            core::ptr::addr_of_mut!((*dst).reuse_address).write(AtomicBool::new(false));
            core::ptr::addr_of_mut!((*dst).broadcast).write(AtomicBool::new(false));
            core::ptr::addr_of_mut!((*dst).ipv6_only).write(AtomicBool::new(false));
            core::ptr::addr_of_mut!((*dst).dont_route).write(AtomicBool::new(false));
            core::ptr::addr_of_mut!((*dst).recv_error).write(AtomicBool::new(false));
            core::ptr::addr_of_mut!((*dst).ip_tos).write(AtomicU32::new(0));
            core::ptr::addr_of_mut!((*dst).pending_error).write(AtomicI32::new(0));
            core::ptr::addr_of_mut!((*dst).linger_enabled).write(AtomicBool::new(false));
            core::ptr::addr_of_mut!((*dst).linger_timeout_secs).write(AtomicU32::new(0));
            core::ptr::addr_of_mut!((*dst).send_timeout_nanos).write(AtomicU64::new(0));
            core::ptr::addr_of_mut!((*dst).recv_timeout_nanos).write(AtomicU64::new(0));
            core::ptr::addr_of_mut!((*dst).send_buffer_len).write(AtomicU32::new(0));
            core::ptr::addr_of_mut!((*dst).recv_buffer_len).write(AtomicU32::new(0));
            core::ptr::addr_of_mut!((*dst).device_mask).write(AtomicU32::new(0));
        }
    }

    pub fn nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) {
        self.nonblock.store(nonblocking, Ordering::Release);
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

    pub fn record_error(&self, error: LinuxError) {
        self.pending_error.store(error.code(), Ordering::Release);
    }

    pub fn clear_error(&self) {
        self.pending_error.store(0, Ordering::Release);
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
        self.send_buffer_len.store(
            Self::normalize_socket_buffer(size) as u32,
            Ordering::Relaxed,
        );
    }

    pub fn set_recv_buffer(&self, size: usize) {
        self.recv_buffer_len.store(
            Self::normalize_socket_buffer(size) as u32,
            Ordering::Relaxed,
        );
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
        let mask = self.device_mask();
        if mask != 0 {
            SERVICE.lock().register_waker(mask, waker);
        }
    }

    pub fn send_poller<P: Pollable, F: FnMut() -> AxResult<T>, T>(
        &self,
        pollable: &P,
        f: F,
    ) -> AxResult<T> {
        block_on(interruptible(timeout(
            self.send_timeout(),
            poll_io(pollable, IoEvents::OUT, self.nonblocking(), f),
        )))??
    }

    pub fn recv_poller<P: Pollable, F: FnMut() -> AxResult<T>, T>(
        &self,
        pollable: &P,
        f: F,
    ) -> AxResult<T> {
        block_on(interruptible(timeout(
            self.recv_timeout(),
            poll_io(pollable, IoEvents::IN, self.nonblocking(), f),
        )))??
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
                block_on(interruptible(timeout(
                    deadline,
                    poll_io(pollable, events, self.nonblocking(), || {
                        // A device interrupt normally wakes this future.  A
                        // bounded timer fallback may do so instead, in which
                        // case the stack must be polled before retrying I/O.
                        poll_interfaces();
                        f()
                    }),
                )))??
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
                **error = self.pending_error.swap(0, Ordering::AcqRel);
            }
            O::NonBlocking(nonblock) => {
                **nonblock = self.nonblocking();
            }
            O::ReuseAddress(reuse) => {
                **reuse = self.reuse_address();
            }
            O::Broadcast(broadcast) => {
                **broadcast = self.broadcast.load(Ordering::Relaxed);
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
            O::TypeOfService(tos) => {
                **tos = self.ip_tos.load(Ordering::Relaxed) as u8;
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
                self.set_nonblocking(*nonblock);
            }
            O::ReuseAddress(reuse) => {
                self.reuse_address.store(*reuse, Ordering::Relaxed);
            }
            O::Broadcast(broadcast) => {
                self.broadcast.store(*broadcast, Ordering::Relaxed);
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
            O::TypeOfService(tos) => {
                self.ip_tos.store(u32::from(*tos), Ordering::Relaxed);
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
