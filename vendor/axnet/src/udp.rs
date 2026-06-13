use alloc::{sync::Arc, vec};
use core::{
    sync::atomic::{AtomicUsize, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError, ax_bail, ax_err_type};
use axio::prelude::*;
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
use smoltcp::{
    iface::SocketHandle,
    phy::PacketMeta,
    socket::udp::{self as smol, UdpMetadata},
    storage::PacketMetadata,
    wire::{IpAddress, IpEndpoint, IpListenEndpoint},
};
use spin::RwLock;

use crate::{
    RecvFlags, RecvOptions, SERVICE, SOCKET_SET, SendOptions, Shutdown, SocketAddrEx, SocketOps,
    consts::{UDP_LOOPBACK_YIELD_BYTES, UDP_PACKET_BUFFER_SIZE, UDP_RX_BUF_LEN, UDP_TX_BUF_LEN},
    family::IpFamily,
    general::GeneralOptions,
    local::LocalUdpSocket,
    options::{Configurable, GetSocketOption, SetSocketOption},
    poll_interfaces,
};

static LOOPBACK_BYTES_SINCE_YIELD: AtomicUsize = AtomicUsize::new(0);

fn maybe_yield_loopback(sent: usize) {
    if sent > 0
        && LOOPBACK_BYTES_SINCE_YIELD.fetch_add(sent, Ordering::Relaxed) + sent
            >= UDP_LOOPBACK_YIELD_BYTES
    {
        LOOPBACK_BYTES_SINCE_YIELD.store(0, Ordering::Relaxed);
        axtask::yield_now();
    }
}

pub(crate) fn new_udp_socket() -> smol::Socket<'static> {
    smol::Socket::new(
        smol::PacketBuffer::new(
            vec![PacketMetadata::EMPTY; UDP_PACKET_BUFFER_SIZE],
            vec![0; UDP_RX_BUF_LEN],
        ),
        smol::PacketBuffer::new(
            vec![PacketMetadata::EMPTY; UDP_PACKET_BUFFER_SIZE],
            vec![0; UDP_TX_BUF_LEN],
        ),
    )
}

/// A UDP socket that provides POSIX-like APIs.
pub struct UdpSocket {
    handle: SocketHandle,
    local: Arc<LocalUdpSocket>,
    family: IpFamily,
    local_addr: RwLock<Option<IpEndpoint>>,
    peer_addr: RwLock<Option<(IpEndpoint, IpAddress)>>,

    general: GeneralOptions,
}

impl UdpSocket {
    /// Creates a new UDP socket.
    #[allow(clippy::new_without_default)]
    pub fn new(family: IpFamily) -> Self {
        let socket = new_udp_socket();
        let handle = SOCKET_SET.add(socket);

        Self {
            handle,
            local: LocalUdpSocket::new(family),
            family,
            local_addr: RwLock::new(None),
            peer_addr: RwLock::new(None),

            general: GeneralOptions::new(),
        }
    }

    fn with_smol_socket<R>(&self, f: impl FnOnce(&mut smol::Socket) -> R) -> R {
        SOCKET_SET.with_socket_mut::<smol::Socket, _, _>(self.handle, f)
    }

    fn configured_rx_buf_len(&self) -> usize {
        self.general.recv_buffer(UDP_RX_BUF_LEN)
    }

    fn configured_tx_buf_len(&self) -> usize {
        self.general.send_buffer(UDP_TX_BUF_LEN)
    }

    fn new_packet_buffer(size: usize) -> smol::PacketBuffer<'static> {
        smol::PacketBuffer::new(
            vec![PacketMetadata::EMPTY; UDP_PACKET_BUFFER_SIZE],
            vec![0; size],
        )
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

            let hop_limit = socket.hop_limit();
            let connected_endpoint = socket.connected_endpoint();
            *socket = smol::Socket::new(
                Self::new_packet_buffer(rx_len),
                Self::new_packet_buffer(tx_len),
            );
            socket.set_hop_limit(hop_limit);
            socket.set_connected_endpoint(connected_endpoint);
            Ok(())
        })
    }

    fn apply_configured_buffers_if_idle(&self) -> AxResult {
        if self.with_smol_socket(|socket| socket.is_open()) {
            return Ok(());
        }
        self.apply_configured_buffers()
    }

    fn remote_endpoint(&self) -> AxResult<(IpEndpoint, IpAddress)> {
        match self.peer_addr.try_read() {
            Some(addr) => addr.ok_or(AxError::NotConnected),
            None => Err(AxError::NotConnected),
        }
    }

    fn endpoint_from_socket_addr(&self, addr: core::net::SocketAddr) -> AxResult<IpEndpoint> {
        self.family
            .endpoint_from_socket_addr(addr, self.general.ipv6_only())
            .ok_or_else(|| AxError::from(LinuxError::EAFNOSUPPORT))
    }

    fn try_loopback_send<S>(
        &self,
        src: S,
        remote_addr: IpEndpoint,
        source_addr: IpAddress,
    ) -> AxResult<Result<usize, S>>
    where
        S: Read + IoBuf,
    {
        if !remote_addr.addr.is_loopback() {
            return Ok(Err(src));
        }

        match self.local.send(src, remote_addr, source_addr)? {
            Ok(sent) => {
                SERVICE.lock().notify_route_device(&remote_addr.addr);
                Ok(Ok(sent))
            }
            Err(src) => Ok(Err(src)),
        }
    }
}

impl Configurable for UdpSocket {
    fn get_option_inner(&self, option: &mut GetSocketOption) -> AxResult<bool> {
        use GetSocketOption as O;

        if self.general.get_option_inner(option)? {
            return Ok(true);
        }
        match option {
            O::Ipv6Only(ipv6_only) if self.family == IpFamily::Ipv6 => {
                **ipv6_only = self.general.ipv6_only();
            }
            O::Ttl(ttl) => {
                self.with_smol_socket(|socket| {
                    **ttl = socket.hop_limit().unwrap_or(64);
                });
            }
            O::SendBuffer(size) | O::SendBufferForce(size) => {
                **size = self.with_smol_socket(|socket| socket.payload_send_capacity());
            }
            O::ReceiveBuffer(size) | O::ReceiveBufferForce(size) => {
                **size = self.with_smol_socket(|socket| socket.payload_recv_capacity());
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
            O::ReceiveBuffer(size) | O::ReceiveBufferForce(size) => {
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
                self.local.set_ipv6_only(*ipv6_only);
            }
            O::Ttl(ttl) => {
                self.with_smol_socket(|socket| {
                    socket.set_hop_limit(Some(*ttl));
                });
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
impl SocketOps for UdpSocket {
    fn bind(&self, local_addr: SocketAddrEx) -> AxResult {
        let mut local_addr = local_addr.into_ip()?;
        let mut local_endpoint = self.endpoint_from_socket_addr(local_addr)?;
        let mut guard = self.local_addr.write();

        if local_addr.port() == 0 {
            local_addr.set_port(get_ephemeral_port()?);
            local_endpoint.port = local_addr.port();
        }
        if guard.is_some() {
            ax_bail!(InvalidInput, "already bound");
        }

        if !SERVICE.lock().can_bind_ip(&local_endpoint.addr) {
            return Err(AxError::from(LinuxError::EADDRNOTAVAIL));
        }
        let endpoint = IpListenEndpoint {
            addr: (!local_endpoint.addr.is_unspecified()).then_some(local_endpoint.addr),
            port: local_endpoint.port,
        };

        if !self.general.reuse_address() {
            // Check if the address is already in use
            SOCKET_SET.bind_check_udp(local_endpoint.addr, local_endpoint.port)?;
        }

        self.apply_configured_buffers()?;
        self.with_smol_socket(|socket| {
            socket.bind(endpoint).map_err(|e| match e {
                smol::BindError::InvalidState => ax_err_type!(InvalidInput, "already bound"),
                smol::BindError::Unaddressable => ax_err_type!(ConnectionRefused, "unaddressable"),
            })
        })?;
        self.local.bind(endpoint)?;
        self.general
            .set_device_mask(SERVICE.lock().device_mask_for(&endpoint));

        *guard = Some(local_endpoint);
        info!("UDP socket {}: bound on {}", self.handle, endpoint);
        Ok(())
    }

    fn connect(&self, remote_addr: SocketAddrEx) -> AxResult {
        let remote_addr = remote_addr.into_ip()?;
        let remote_addr = self.endpoint_from_socket_addr(remote_addr)?;
        let mut guard = self.peer_addr.write();
        if self.local_addr.read().is_none() {
            self.bind(SocketAddrEx::Ip(
                self.family
                    .format_socket_addr(self.family.unspecified_addr(), 0),
            ))?;
        }

        let src = SERVICE.lock().get_source_address(&remote_addr.addr)?;
        self.with_smol_socket(|socket| {
            socket.set_connected_endpoint(Some(remote_addr));
        });
        self.local.connect(remote_addr);
        *guard = Some((remote_addr, src));
        debug!("UDP socket {}: connected to {}", self.handle, remote_addr);
        Ok(())
    }

    fn send(&self, mut src: impl Read + IoBuf, options: SendOptions) -> AxResult<usize> {
        let (remote_addr, source_addr) = match options.to {
            Some(addr) => {
                let addr = addr.into_ip()?;
                let addr = self.endpoint_from_socket_addr(addr)?;
                let src = SERVICE.lock().get_source_address(&addr.addr)?;
                (addr, src)
            }
            None => self.remote_endpoint()?,
        };
        if remote_addr.port == 0 || remote_addr.addr.is_unspecified() {
            ax_bail!(InvalidInput, "invalid address");
        }

        if self.local_addr.read().is_none() {
            self.bind(SocketAddrEx::Ip(
                self.family
                    .format_socket_addr(self.family.unspecified_addr(), 0),
            ))?;
        }
        let is_loopback = remote_addr.addr.is_loopback();
        match self.try_loopback_send(src, remote_addr, source_addr)? {
            Ok(sent) => {
                maybe_yield_loopback(sent);
                return Ok(sent);
            }
            Err(remaining) => src = remaining,
        }

        let sent = self.general.network_send_poller(self, || {
            self.with_smol_socket(|socket| {
                if !socket.is_open() {
                    // not connected
                    Err(ax_err_type!(NotConnected))
                } else if !socket.can_send() {
                    Err(AxError::WouldBlock)
                } else {
                    let buf = socket
                        .send(
                            src.remaining(),
                            UdpMetadata {
                                endpoint: remote_addr,
                                local_address: Some(source_addr),
                                meta: PacketMeta::default(),
                            },
                        )
                        .map_err(|e| match e {
                            smol::SendError::BufferFull => AxError::WouldBlock,
                            smol::SendError::Unaddressable => {
                                ax_err_type!(ConnectionRefused, "unaddressable")
                            }
                        })?;
                    let read = src.read(buf)?;
                    assert_eq!(read, buf.len());
                    Ok(read)
                }
            })
        })?;
        if sent > 0 {
            poll_interfaces();
            if is_loopback {
                maybe_yield_loopback(sent);
            }
        }
        Ok(sent)
    }

    fn recv(&self, mut dst: impl Write, mut options: RecvOptions) -> AxResult<usize> {
        if self.local_addr.read().is_none() {
            ax_bail!(NotConnected);
        }

        let flags = options.flags;
        let mut from = options.from.take();

        self.general.network_recv_poller(self, || {
            let local_options = RecvOptions {
                from: from.as_mut().map(|addr| &mut **addr),
                flags,
                cmsg: None,
            };
            match self.local.recv(&mut dst, self.family, local_options) {
                Ok(read) => return Ok(read),
                Err(AxError::WouldBlock) | Err(AxError::NotConnected) => {}
                Err(err) => return Err(err),
            }

            enum ExpectedRemote<'a> {
                Any(&'a mut SocketAddrEx),
                Expecting(IpEndpoint),
                Unspecified,
            }
            let mut expected_remote = match from.as_mut() {
                Some(addr) => ExpectedRemote::Any(&mut **addr),
                None => match self.remote_endpoint() {
                    Ok((endpoint, _)) => ExpectedRemote::Expecting(endpoint),
                    Err(AxError::NotConnected) => ExpectedRemote::Unspecified,
                    Err(e) => return Err(e),
                },
            };

            self.with_smol_socket(|socket| {
                if !socket.is_open() {
                    // not bound
                    Err(ax_err_type!(NotConnected))
                } else if !socket.can_recv() {
                    Err(AxError::WouldBlock)
                } else {
                    let result = if flags.contains(RecvFlags::PEEK) {
                        socket.peek().map(|(data, meta)| (data, *meta))
                    } else {
                        socket.recv()
                    };
                    match result {
                        Ok((src, meta)) => {
                            match &mut expected_remote {
                                ExpectedRemote::Any(remote_addr) => {
                                    **remote_addr =
                                        SocketAddrEx::Ip(self.family.format_socket_addr(
                                            meta.endpoint.addr,
                                            meta.endpoint.port,
                                        ));
                                }
                                ExpectedRemote::Expecting(expected) => {
                                    if (!expected.addr.is_unspecified()
                                        && expected.addr != meta.endpoint.addr)
                                        || (expected.port != 0
                                            && expected.port != meta.endpoint.port)
                                    {
                                        return Err(AxError::WouldBlock);
                                    }
                                }
                                ExpectedRemote::Unspecified => {}
                            }

                            let read = dst.write(src)?;
                            if read < src.len() {
                                warn!("UDP message truncated: {} -> {} bytes", src.len(), read);
                            }

                            Ok(if flags.contains(RecvFlags::TRUNCATE) {
                                src.len()
                            } else {
                                read
                            })
                        }
                        Err(smol::RecvError::Exhausted) => Err(AxError::WouldBlock),
                        Err(smol::RecvError::Truncated) => {
                            unreachable!("UDP socket recv never returns Err(Truncated)")
                        }
                    }
                }
            })
        })
    }

    fn local_addr(&self) -> AxResult<SocketAddrEx> {
        match self.local_addr.try_read() {
            Some(addr) => addr
                .map(|addr| self.family.format_socket_addr(addr.addr, addr.port))
                .map(SocketAddrEx::Ip)
                .ok_or(AxError::NotConnected),
            None => Err(AxError::NotConnected),
        }
    }

    fn peer_addr(&self) -> AxResult<SocketAddrEx> {
        self.remote_endpoint()
            .map(|it| self.family.format_socket_addr(it.0.addr, it.0.port))
            .map(SocketAddrEx::Ip)
    }

    fn shutdown(&self, _how: Shutdown) -> AxResult {
        // TODO(mivik): shutdown
        poll_interfaces();

        self.with_smol_socket(|socket| {
            debug!("UDP socket {}: shutting down", self.handle);
            socket.close();
        });
        self.local.close();
        Ok(())
    }
}

impl Pollable for UdpSocket {
    fn poll(&self) -> IoEvents {
        if self.local_addr.read().is_none() {
            return IoEvents::empty();
        }

        let mut events = IoEvents::empty();
        let local_events = self.local.poll();
        self.with_smol_socket(|socket| {
            events.set(
                IoEvents::IN,
                socket.can_recv() || local_events.contains(IoEvents::IN),
            );
            events.set(IoEvents::OUT, socket.can_send());
        });
        events
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.intersects(IoEvents::IN | IoEvents::OUT) {
            self.general.register_waker(context.waker());
            self.local.register(context.waker());
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        self.shutdown(Shutdown::Both).ok();
        SOCKET_SET.remove(self.handle);
    }
}

fn get_ephemeral_port() -> AxResult<u16> {
    const PORT_START: u16 = 0xc000;
    const PORT_END: u16 = 0xffff;
    static CURR: Mutex<u16> = Mutex::new(PORT_START);
    let mut curr = CURR.lock();

    let port = *curr;
    if *curr == PORT_END {
        *curr = PORT_START;
    } else {
        *curr += 1;
    }
    Ok(port)
}
