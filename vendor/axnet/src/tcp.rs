use alloc::vec;
use core::{
    sync::atomic::{AtomicBool, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError, ax_bail, ax_err_type};
use axio::prelude::*;
use axpoll::{IoEvents, PollSet, Pollable};
use axsync::Mutex;
use smoltcp::{
    iface::SocketHandle,
    socket::tcp as smol,
    time::Duration,
    wire::{IpAddress, IpEndpoint, IpListenEndpoint, TCP_HEADER_LEN},
};

use super::{LISTEN_TABLE, SOCKET_SET};
use crate::{
    RecvFlags, RecvOptions, SERVICE, SendOptions, Shutdown, Socket, SocketAddrEx, SocketOps,
    consts::{
        LOOPBACK_ADVERTISED_MSS, LOOPBACK_MTU, STANDARD_MTU, TCP_RX_BUF_LEN, TCP_TX_BUF_LEN,
    },
    family::IpFamily,
    general::GeneralOptions,
    options::{Configurable, GetSocketOption, SetSocketOption, TcpInfo},
    poll_interfaces,
    state::*,
};

pub(crate) fn new_tcp_socket() -> smol::Socket<'static> {
    smol::Socket::new(
        smol::SocketBuffer::new(vec![0; TCP_RX_BUF_LEN]),
        smol::SocketBuffer::new(vec![0; TCP_TX_BUF_LEN]),
    )
}

/// A TCP socket that provides POSIX-like APIs.
pub struct TcpSocket {
    state: StateLock,
    handle: SocketHandle,
    family: IpFamily,
    ever_connected: AtomicBool,

    general: GeneralOptions,
    has_mcast_group: AtomicBool,
    rx_closed: AtomicBool,
    poll_rx_closed: PollSet,
}

unsafe impl Sync for TcpSocket {}

impl TcpSocket {
    /// Creates a new TCP socket.
    pub fn new(family: IpFamily) -> Self {
        Self {
            state: StateLock::new(State::Idle),
            handle: SOCKET_SET.add(new_tcp_socket()),
            family,
            ever_connected: AtomicBool::new(false),

            general: GeneralOptions::new(),
            has_mcast_group: AtomicBool::new(false),
            rx_closed: AtomicBool::new(false),
            poll_rx_closed: PollSet::new(),
        }
    }

    /// Creates a new TCP socket that is already connected.
    fn new_connected(
        handle: SocketHandle,
        family: IpFamily,
        rx_buf_len: usize,
        tx_buf_len: usize,
    ) -> Self {
        let result = Self {
            state: StateLock::new(State::Connected),
            handle,
            family,
            ever_connected: AtomicBool::new(true),

            general: GeneralOptions::new(),
            has_mcast_group: AtomicBool::new(false),
            rx_closed: AtomicBool::new(false),
            poll_rx_closed: PollSet::new(),
        };
        result.general.set_recv_buffer(rx_buf_len);
        result.general.set_send_buffer(tx_buf_len);
        let endpoint = result.with_smol_socket(|socket| {
            socket
                .local_endpoint()
                .map(|endpoint| IpListenEndpoint {
                    addr: Some(endpoint.addr),
                    port: endpoint.port,
                })
                .unwrap_or_else(|| socket.get_bound_endpoint())
        });
        result
            .general
            .set_device_mask(SERVICE.lock().device_mask_for(&endpoint));
        result
    }
}

/// Private methods
impl TcpSocket {
    fn state(&self) -> State {
        self.state.get()
    }

    #[inline]
    fn is_listening(&self) -> bool {
        self.state() == State::Listening
    }

    fn with_smol_socket<R>(&self, f: impl FnOnce(&mut smol::Socket) -> R) -> R {
        SOCKET_SET.with_socket_mut::<smol::Socket, _, _>(self.handle, f)
    }

    fn configured_rx_buf_len(&self) -> usize {
        self.general.recv_buffer(TCP_RX_BUF_LEN)
    }

    fn configured_tx_buf_len(&self) -> usize {
        self.general.send_buffer(TCP_TX_BUF_LEN)
    }

    fn apply_configured_buffers(&self) -> AxResult {
        let rx_len = self.configured_rx_buf_len();
        let tx_len = self.configured_tx_buf_len();
        self.with_smol_socket(|socket| {
            if socket.is_open() {
                return Err(ax_err_type!(
                    InvalidInput,
                    "socket buffers are already active"
                ));
            }

            let nagle = socket.nagle_enabled();
            let keep_alive = socket.keep_alive();
            let timeout = socket.timeout();
            let ack_delay = socket.ack_delay();
            let hop_limit = socket.hop_limit();
            let bound_endpoint = socket.get_bound_endpoint();

            *socket = smol::Socket::new(
                smol::SocketBuffer::new(vec![0; rx_len]),
                smol::SocketBuffer::new(vec![0; tx_len]),
            );
            socket.set_nagle_enabled(nagle);
            socket.set_keep_alive(keep_alive);
            socket.set_timeout(timeout);
            socket.set_ack_delay(ack_delay);
            socket.set_hop_limit(hop_limit);
            socket.set_bound_endpoint(bound_endpoint);
            Ok(())
        })
    }

    fn apply_configured_buffers_if_idle(&self) -> AxResult {
        if self.with_smol_socket(|socket| socket.is_open()) {
            return Ok(());
        }
        self.apply_configured_buffers()
    }

    fn path_mtu_and_mss(&self) -> (usize, usize) {
        let addr = self.with_smol_socket(|socket| {
            socket
                .remote_endpoint()
                .map(|endpoint| endpoint.addr)
                .or_else(|| socket.get_bound_endpoint().addr)
        });
        let addr = addr.unwrap_or_else(|| self.family.unspecified_addr());
        let mtu = if addr.is_loopback() {
            LOOPBACK_MTU
        } else {
            STANDARD_MTU
        };
        let ip_header_len = match addr {
            IpAddress::Ipv4(_) => 20,
            IpAddress::Ipv6(_) => 40,
        };
        (mtu, mtu.saturating_sub(ip_header_len + TCP_HEADER_LEN))
    }

    fn advertised_path_mtu_and_mss(&self) -> (usize, usize) {
        let (pmtu, mss) = self.path_mtu_and_mss();
        (pmtu, mss.min(LOOPBACK_ADVERTISED_MSS))
    }

    fn endpoint_from_socket_addr(&self, addr: core::net::SocketAddr) -> AxResult<IpEndpoint> {
        self.family
            .endpoint_from_socket_addr(addr, self.general.ipv6_only())
            .ok_or_else(|| AxError::from(LinuxError::EAFNOSUPPORT))
    }

    fn bound_endpoint(&self) -> AxResult<IpListenEndpoint> {
        let endpoint = self.with_smol_socket(|socket| socket.get_bound_endpoint());
        if endpoint.port == 0 {
            ax_bail!(InvalidInput, "not bound");
        }
        Ok(endpoint)
    }

    fn poll_connect(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        let writable = self.with_smol_socket(|socket| match socket.state() {
            smol::State::SynSent => false, // wait for connection
            smol::State::Established => {
                self.state.set(State::Connected); // connected
                self.ever_connected.store(true, Ordering::Release);
                debug!(
                    "TCP socket {}: connected to {}",
                    self.handle,
                    socket.remote_endpoint().unwrap(),
                );
                true
            }
            _ => {
                self.state.set(State::Closed); // connection failed
                true
            }
        });
        events.set(IoEvents::OUT, writable);
        events
    }

    fn poll_stream(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        self.with_smol_socket(|socket| {
            events.set(
                IoEvents::IN,
                !self.rx_closed.load(Ordering::Acquire)
                    && (!socket.may_recv() || socket.can_recv()),
            );
            events.set(IoEvents::OUT, !socket.may_send() || socket.can_send());
        });
        events
    }

    fn poll_listener(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        events.set(
            IoEvents::IN,
            LISTEN_TABLE
                .can_accept(self.bound_endpoint().unwrap().port)
                .unwrap(),
        );
        events
    }
}

impl Configurable for TcpSocket {
    fn get_option_inner(&self, option: &mut GetSocketOption) -> AxResult<bool> {
        use GetSocketOption as O;

        if self.general.get_option_inner(option)? {
            return Ok(true);
        }

        match option {
            O::Ipv6Only(ipv6_only) if self.family == IpFamily::Ipv6 => {
                **ipv6_only = self.general.ipv6_only();
            }
            O::NoDelay(no_delay) => {
                **no_delay = self.with_smol_socket(|socket| !socket.nagle_enabled());
            }
            O::KeepAlive(keep_alive) => {
                **keep_alive = self.with_smol_socket(|socket| socket.keep_alive().is_some());
            }
            O::MaxSegment(max_segment) => {
                **max_segment = self.advertised_path_mtu_and_mss().1;
            }
            O::SendBuffer(size) => {
                **size = self.with_smol_socket(|socket| socket.send_capacity());
            }
            O::ReceiveBuffer(size) => {
                **size = self.with_smol_socket(|socket| socket.recv_capacity());
            }
            O::TcpInfo(info) => {
                let (pmtu, mss) = self.advertised_path_mtu_and_mss();
                // Minimal Linux-compatible TCP_INFO: report deterministic values
                // so user space does not consume stale stack contents.
                **info = TcpInfo {
                    tcpi_state: self.with_smol_socket(|socket| tcp_state_number(socket.state())),
                    tcpi_snd_mss: mss as u32,
                    tcpi_rcv_mss: mss as u32,
                    tcpi_snd_ssthresh: u32::MAX,
                    tcpi_snd_cwnd: 10,
                    tcpi_advmss: mss as u32,
                    tcpi_pmtu: pmtu as u32,
                    ..TcpInfo::default()
                };
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn set_option_inner(&self, option: SetSocketOption) -> AxResult<bool> {
        use SetSocketOption as O;

        match option {
            O::SendBuffer(size) | O::SendBufferForce(size) => {
                self.general.set_send_buffer(*size);
                self.apply_configured_buffers_if_idle()?;
                return Ok(true);
            }
            O::ReceiveBuffer(size) => {
                self.general.set_recv_buffer(*size);
                self.apply_configured_buffers_if_idle()?;
                return Ok(true);
            }
            _ => {}
        }

        if self.general.set_option_inner(option)? {
            return Ok(true);
        }

        match option {
            O::Ipv6Only(ipv6_only) if self.family == IpFamily::Ipv6 => {
                self.general.set_ipv6_only(*ipv6_only);
            }
            O::NoDelay(no_delay) => {
                self.with_smol_socket(|socket| {
                    socket.set_nagle_enabled(!no_delay);
                });
            }
            O::KeepAlive(keep_alive) => {
                self.with_smol_socket(|socket| {
                    socket.set_keep_alive(keep_alive.then(|| Duration::from_secs(75)));
                });
            }
            O::McastGroupOp(join) => {
                if *join {
                    self.has_mcast_group.store(true, Ordering::Release);
                } else if !self.has_mcast_group.swap(false, Ordering::AcqRel) {
                    return Err(AxError::from(LinuxError::EADDRNOTAVAIL));
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
impl SocketOps for TcpSocket {
    fn bind(&self, local_addr: SocketAddrEx) -> AxResult {
        let mut local_addr = local_addr.into_ip()?;
        let mut local_endpoint = self.endpoint_from_socket_addr(local_addr)?;
        self.state
            .lock(State::Idle)
            .map_err(|_| ax_err_type!(InvalidInput, "already bound"))?
            .transit(State::Idle, || {
                if local_addr.port() == 0 {
                    local_addr.set_port(get_ephemeral_port()?);
                    local_endpoint.port = local_addr.port();
                }
                if !SERVICE.lock().can_bind_ip(&local_endpoint.addr) {
                    return Err(AxError::from(LinuxError::EADDRNOTAVAIL));
                }
                if !self.general.reuse_address() {
                    SOCKET_SET.bind_check_tcp(local_endpoint.addr, local_endpoint.port)?;
                }

                self.with_smol_socket(|socket| {
                    if socket.get_bound_endpoint().port != 0 {
                        return Err(AxError::InvalidInput);
                    }
                    let endpoint = IpListenEndpoint {
                        addr: if local_endpoint.addr.is_unspecified() {
                            None
                        } else {
                            Some(local_endpoint.addr)
                        },
                        port: local_endpoint.port,
                    };
                    socket.set_bound_endpoint(endpoint);
                    self.general
                        .set_device_mask(SERVICE.lock().device_mask_for(&endpoint));
                    Ok(())
                })?;
                debug!("TCP socket {}: binding to {}", self.handle, local_addr);
                Ok(())
            })
    }

    fn connect(&self, remote_addr: SocketAddrEx) -> AxResult {
        let remote_addr = remote_addr.into_ip()?;
        let remote_endpoint = self.endpoint_from_socket_addr(remote_addr)?;
        if self.ever_connected.load(Ordering::Acquire) {
            return Err(ax_err_type!(AlreadyConnected));
        }
        self.state
            .lock(State::Idle)
            .map_err(|state| {
                if state == State::Connecting {
                    AxError::InProgress
                } else {
                    // TODO(mivik): error code
                    ax_err_type!(AlreadyConnected)
                }
            })?
            .transit(State::Connecting, || {
                self.apply_configured_buffers()?;
                // TODO: check remote addr unreachable
                // let (bound_endpoint, remote_endpoint) = self.get_endpoint_pair(remote_addr)?;
                let mut bound_endpoint =
                    self.with_smol_socket(|socket| socket.get_bound_endpoint());
                if bound_endpoint.addr.is_none() {
                    bound_endpoint.addr =
                        Some(SERVICE.lock().get_source_address(&remote_endpoint.addr)?);
                }
                if bound_endpoint.port == 0 {
                    bound_endpoint.port = get_ephemeral_port()?;
                }
                info!(
                    "TCP connection from {} to {}",
                    bound_endpoint, remote_endpoint
                );

                self.with_smol_socket(|socket| {
                    socket.set_bound_endpoint(bound_endpoint);
                    self.general
                        .set_device_mask(SERVICE.lock().device_mask_for(&bound_endpoint));
                    socket
                        .connect(
                            crate::SERVICE.lock().iface.context(),
                            remote_endpoint,
                            bound_endpoint,
                        )
                        .map_err(|e| match e {
                            smol::ConnectError::InvalidState => {
                                ax_err_type!(AlreadyConnected)
                            }
                            smol::ConnectError::Unaddressable => {
                                ax_err_type!(ConnectionRefused, "unaddressable")
                            }
                        })?;
                    Ok(())
                })
            })?;

        // Hack: let the server listen
        axtask::yield_now();

        // Here our state must be `CONNECTING`, and only one thread can run here.
        self.general.send_poller(self, || {
            poll_interfaces();
            let events = self.poll_connect();
            if !events.contains(IoEvents::OUT) {
                Err(AxError::WouldBlock)
            } else if self.state() == State::Connected {
                Ok(())
            } else {
                Err(ax_err_type!(ConnectionRefused, "connection refused"))
            }
        })
    }

    fn listen(&self) -> AxResult {
        if let Ok(guard) = self.state.lock(State::Idle) {
            guard.transit(State::Listening, || {
                self.apply_configured_buffers()?;
                let bound_endpoint = self.with_smol_socket(|socket| socket.get_bound_endpoint());
                LISTEN_TABLE.listen(
                    bound_endpoint,
                    self.family,
                    self.general.ipv6_only(),
                    self.configured_rx_buf_len(),
                    self.configured_tx_buf_len(),
                )?;
                debug!("listening on {}", bound_endpoint);
                Ok(())
            })?;
        } else {
            // ignore simultaneous `listen`s.
        }
        Ok(())
    }

    fn accept(&self) -> AxResult<Socket> {
        if !self.is_listening() {
            ax_bail!(InvalidInput, "not listening");
        }

        let bound_port = self.bound_endpoint()?.port;
        self.general.recv_poller(self, || {
            poll_interfaces();
            LISTEN_TABLE.accept(bound_port).map(|handle| {
                let socket = TcpSocket::new_connected(
                    handle,
                    self.family,
                    self.configured_rx_buf_len(),
                    self.configured_tx_buf_len(),
                );
                debug!(
                    "accepted connection from {}, {}",
                    handle,
                    socket.with_smol_socket(|socket| socket.remote_endpoint().unwrap())
                );
                Socket::Tcp(socket)
            })
        })
    }

    fn send(&self, mut src: impl Read, _options: SendOptions) -> AxResult<usize> {
        // SAFETY: `self.handle` should be initialized in a connected socket.
        let sent = self.general.network_send_poller(self, || {
            self.with_smol_socket(|socket| {
                if !socket.is_active() {
                    Err(AxError::NotConnected)
                } else if !socket.can_send() {
                    Err(AxError::WouldBlock)
                } else {
                    // connected, and the tx buffer is not full
                    let len = socket
                        .send(|buffer| {
                            let result = src.read(buffer);
                            let len = result.unwrap_or(0);
                            (len, result)
                        })
                        .map_err(|_| ax_err_type!(NotConnected, "not connected?"))??;
                    Ok(len)
                }
            })
        })?;
        if sent > 0 {
            poll_interfaces();
        }
        Ok(sent)
    }

    fn recv(&self, mut dst: impl Write + IoBufMut, options: RecvOptions<'_>) -> AxResult<usize> {
        if self.rx_closed.load(Ordering::Acquire) {
            return Err(AxError::NotConnected);
        }
        self.general.network_recv_poller(self, || {
            self.with_smol_socket(|socket| {
                if !socket.is_active() {
                    Err(AxError::NotConnected)
                } else if !socket.may_recv() {
                    Ok(0)
                } else if socket.recv_queue() == 0 {
                    Err(AxError::WouldBlock)
                } else if options.flags.contains(RecvFlags::PEEK) {
                    dst.write(
                        socket
                            .peek(dst.remaining_mut())
                            .map_err(|_| ax_err_type!(NotConnected, "not connected?"))?,
                    )
                } else {
                    socket
                        .recv(|buf| {
                            let result = dst.write(buf);
                            let len = result.unwrap_or(0);
                            (len, result)
                        })
                        .map_err(|_| ax_err_type!(NotConnected, "not connected?"))?
                }
            })
        })
    }

    fn local_addr(&self) -> AxResult<SocketAddrEx> {
        self.with_smol_socket(|socket| {
            let (addr, port) = if let Some(endpoint) = socket.local_endpoint() {
                (endpoint.addr, endpoint.port)
            } else {
                let endpoint = socket.get_bound_endpoint();
                (
                    endpoint.addr.unwrap_or_else(|| self.family.unspecified_addr()),
                    endpoint.port,
                )
            };
            Ok(SocketAddrEx::Ip(
                self.family.format_socket_addr(addr, port),
            ))
        })
    }

    fn peer_addr(&self) -> AxResult<SocketAddrEx> {
        self.with_smol_socket(|socket| {
            let endpoint = socket.remote_endpoint().ok_or(AxError::NotConnected)?;
            Ok(SocketAddrEx::Ip(
                self.family
                    .format_socket_addr(endpoint.addr, endpoint.port),
            ))
        })
    }

    fn shutdown(&self, how: Shutdown) -> AxResult {
        // TODO(mivik): shutdown
        if how.has_read() {
            self.rx_closed.store(true, Ordering::Release);
            self.poll_rx_closed.wake();
        }

        // stream
        if let Ok(guard) = self.state.lock(State::Connected) {
            guard.transit(State::Closed, || {
                if how.has_write() {
                    self.with_smol_socket(|socket| {
                        debug!("TCP socket {}: shutting down", self.handle);
                        socket.close();
                    });
                }
                poll_interfaces();
                Ok(())
            })?;
        }

        // listener
        if let Ok(guard) = self.state.lock(State::Listening) {
            guard.transit(State::Closed, || {
                LISTEN_TABLE.unlisten(self.bound_endpoint()?.port);
                poll_interfaces();
                Ok(())
            })?;
        }

        // ignore for other states
        Ok(())
    }
}

fn tcp_state_number(state: smol::State) -> u8 {
    match state {
        smol::State::Closed => 7,
        smol::State::Listen => 10,
        smol::State::SynSent => 2,
        smol::State::SynReceived => 3,
        smol::State::Established => 1,
        smol::State::FinWait1 => 4,
        smol::State::FinWait2 => 5,
        smol::State::CloseWait => 8,
        smol::State::Closing => 11,
        smol::State::LastAck => 9,
        smol::State::TimeWait => 6,
    }
}

impl Pollable for TcpSocket {
    fn poll(&self) -> IoEvents {
        let mut events = match self.state() {
            State::Connecting => self.poll_connect(),
            State::Connected | State::Idle | State::Closed => self.poll_stream(),
            State::Listening => self.poll_listener(),
            State::Busy => IoEvents::empty(),
        };
        events.set(IoEvents::RDHUP, self.rx_closed.load(Ordering::Acquire));
        events
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.intersects(IoEvents::IN | IoEvents::OUT | IoEvents::RDHUP) {
            self.general.register_waker(context.waker());
        }
        if events.contains(IoEvents::RDHUP) {
            self.poll_rx_closed.register(context.waker());
        }
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        if let Err(err) = self.shutdown(Shutdown::Both) {
            warn!("TCP socket {}: shutdown failed: {}", self.handle, err);
        }
        SOCKET_SET.remove(self.handle);
        // This is crucial for the close messages to be sent.
        poll_interfaces();
    }
}

fn get_ephemeral_port() -> AxResult<u16> {
    const PORT_START: u16 = 0xc000;
    const PORT_END: u16 = 0xffff;
    static CURR: Mutex<u16> = Mutex::new(PORT_START);

    let mut curr = CURR.lock();
    let mut tries = 0;
    // TODO: more robust
    while tries <= PORT_END - PORT_START {
        let port = *curr;
        if *curr == PORT_END {
            *curr = PORT_START;
        } else {
            *curr += 1;
        }
        if LISTEN_TABLE.can_listen(port) {
            return Ok(port);
        }
        tries += 1;
    }
    ax_bail!(AddrInUse, "no available ports");
}
