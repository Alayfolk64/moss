use alloc::{boxed::Box, vec, vec::Vec};
use core::{
    net::SocketAddr,
    ops::Deref,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axio::prelude::*;
use axpoll::{IoEvents, Pollable};
use smoltcp::{
    iface::SocketHandle,
    socket::raw as smol,
    storage::PacketMetadata,
    wire::{
        IPV4_HEADER_LEN, IPV6_HEADER_LEN, Icmpv4Packet, Icmpv6Packet, IpAddress, IpListenEndpoint,
        IpProtocol, IpVersion, Ipv4Packet, Ipv4Repr, Ipv6Packet, Ipv6Repr,
    },
};
use spin::RwLock;

use crate::{
    RecvControlMessage, RecvFlags, RecvOptions, SERVICE, SOCKET_SET, SendOptions, Shutdown,
    SocketAddrEx, SocketOps,
    consts::{RAW_PACKET_BUFFER_SIZE, RAW_RX_BUF_LEN, RAW_TX_BUF_LEN},
    family::IpFamily,
    general::GeneralOptions,
    options::{Configurable, GetSocketOption, SetSocketOption},
    poll_interfaces,
};

fn new_packet_buffer(size: usize) -> smol::PacketBuffer<'static> {
    smol::PacketBuffer::new(
        vec![PacketMetadata::EMPTY; RAW_PACKET_BUFFER_SIZE],
        vec![0; size],
    )
}

fn new_raw_socket(family: IpFamily, protocol: IpProtocol) -> smol::Socket<'static> {
    let version = match family {
        IpFamily::Ipv4 => IpVersion::Ipv4,
        IpFamily::Ipv6 => IpVersion::Ipv6,
    };
    smol::Socket::new(
        Some(version),
        Some(protocol),
        new_packet_buffer(RAW_RX_BUF_LEN),
        new_packet_buffer(RAW_TX_BUF_LEN),
    )
}

/// Heap-resident state of a [`RawSocket`].
pub struct RawSocketInner {
    handle: SocketHandle,
    family: IpFamily,
    protocol: IpProtocol,
    local_addr: RwLock<Option<IpAddress>>,
    peer_addr: RwLock<Option<IpAddress>>,
    hop_limit: AtomicU8,
    ipv6_hop_limit_cmsg: AtomicU32,
    icmpv6_filter: RwLock<[u32; 8]>,
    general: GeneralOptions,
}

/// A Linux-compatible raw IP socket used by ordinary ICMP applications.
///
/// Keep the state in its final allocation.  Apart from making moves cheap,
/// this prevents large aggregate moves from corrupting atomic socket options
/// on affected LoongArch compiler versions.
pub struct RawSocket(Box<RawSocketInner>);

impl Deref for RawSocket {
    type Target = RawSocketInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

const _: () = assert!(core::mem::size_of::<RawSocket>() == core::mem::size_of::<usize>());

impl RawSocket {
    pub fn new_icmp(family: IpFamily) -> Self {
        let protocol = match family {
            IpFamily::Ipv4 => IpProtocol::Icmp,
            IpFamily::Ipv6 => IpProtocol::Icmpv6,
        };
        let handle = SOCKET_SET.add(new_raw_socket(family, protocol));
        let mut storage = Box::<RawSocketInner>::new_uninit();
        let dst = storage.as_mut_ptr();

        unsafe {
            core::ptr::addr_of_mut!((*dst).handle).write(handle);
            core::ptr::addr_of_mut!((*dst).family).write(family);
            core::ptr::addr_of_mut!((*dst).protocol).write(protocol);
            core::ptr::addr_of_mut!((*dst).local_addr).write(RwLock::new(None));
            core::ptr::addr_of_mut!((*dst).peer_addr).write(RwLock::new(None));
            core::ptr::addr_of_mut!((*dst).hop_limit).write(AtomicU8::new(64));
            core::ptr::addr_of_mut!((*dst).ipv6_hop_limit_cmsg).write(AtomicU32::new(0));
            core::ptr::addr_of_mut!((*dst).icmpv6_filter).write(RwLock::new([0; 8]));
            GeneralOptions::write_unpublished_in_place(core::ptr::addr_of_mut!((*dst).general));
            Self(storage.assume_init())
        }
    }

    fn with_socket<R>(&self, f: impl FnOnce(&mut smol::Socket) -> R) -> R {
        SOCKET_SET.with_socket_mut::<smol::Socket, _, _>(self.handle, f)
    }

    fn endpoint_address(&self, address: SocketAddr) -> AxResult<IpAddress> {
        self.family
            .endpoint_from_socket_addr(address, true)
            .map(|endpoint| endpoint.addr)
            .ok_or_else(|| AxError::from(LinuxError::EAFNOSUPPORT))
    }

    fn format_address(&self, address: IpAddress) -> SocketAddrEx {
        SocketAddrEx::Ip(self.family.format_socket_addr(address, 0))
    }

    fn source_address(&self, destination: IpAddress) -> AxResult<IpAddress> {
        if let Some(address) = *self.local_addr.read()
            && !address.is_unspecified()
        {
            return Ok(address);
        }
        SERVICE.lock().get_source_address(&destination)
    }

    fn prepare_packet(&self, payload: &[u8], destination: IpAddress) -> AxResult<Vec<u8>> {
        let source = self.source_address(destination)?;
        let hop_limit = self.hop_limit.load(Ordering::Relaxed);
        match (source, destination) {
            (IpAddress::Ipv4(source), IpAddress::Ipv4(destination)) => {
                let packet_len = IPV4_HEADER_LEN
                    .checked_add(payload.len())
                    .ok_or_else(|| AxError::from(LinuxError::EMSGSIZE))?;
                if packet_len > usize::from(u16::MAX) {
                    return Err(AxError::from(LinuxError::EMSGSIZE));
                }
                let mut bytes = vec![0; packet_len];
                let repr = Ipv4Repr {
                    src_addr: source,
                    dst_addr: destination,
                    next_header: self.protocol,
                    payload_len: payload.len(),
                    hop_limit,
                };
                let mut packet = Ipv4Packet::new_unchecked(&mut bytes);
                repr.emit(&mut packet, &Default::default());
                packet.payload_mut().copy_from_slice(payload);
                if self.protocol == IpProtocol::Icmp {
                    Icmpv4Packet::new_checked(packet.payload_mut())
                        .map_err(|_| AxError::InvalidInput)?
                        .fill_checksum();
                }
                Ok(bytes)
            }
            (IpAddress::Ipv6(source), IpAddress::Ipv6(destination)) => {
                if payload.len() > usize::from(u16::MAX) {
                    return Err(AxError::from(LinuxError::EMSGSIZE));
                }
                let packet_len = IPV6_HEADER_LEN
                    .checked_add(payload.len())
                    .ok_or_else(|| AxError::from(LinuxError::EMSGSIZE))?;
                let mut bytes = vec![0; packet_len];
                let repr = Ipv6Repr {
                    src_addr: source,
                    dst_addr: destination,
                    next_header: self.protocol,
                    payload_len: payload.len(),
                    hop_limit,
                };
                let mut packet = Ipv6Packet::new_unchecked(&mut bytes);
                repr.emit(&mut packet);
                packet.payload_mut().copy_from_slice(payload);
                if self.protocol == IpProtocol::Icmpv6 {
                    Icmpv6Packet::new_checked(packet.payload_mut())
                        .map_err(|_| AxError::InvalidInput)?
                        .fill_checksum(&source, &destination);
                }
                Ok(bytes)
            }
            _ => Err(AxError::from(LinuxError::EAFNOSUPPORT)),
        }
    }

    fn parse_received<'a>(&self, packet: &'a [u8]) -> AxResult<(IpAddress, u8, &'a [u8])> {
        match self.family {
            IpFamily::Ipv4 => {
                let packet = Ipv4Packet::new_checked(packet).map_err(|_| AxError::InvalidData)?;
                Ok((
                    IpAddress::Ipv4(packet.src_addr()),
                    packet.hop_limit(),
                    packet.into_inner(),
                ))
            }
            IpFamily::Ipv6 => {
                let packet = Ipv6Packet::new_checked(packet).map_err(|_| AxError::InvalidData)?;
                Ok((
                    IpAddress::Ipv6(packet.src_addr()),
                    packet.hop_limit(),
                    packet.payload(),
                ))
            }
        }
    }

    fn icmpv6_filtered(&self, payload: &[u8]) -> bool {
        if self.protocol != IpProtocol::Icmpv6 || payload.is_empty() {
            return false;
        }
        let message_type = usize::from(payload[0]);
        self.icmpv6_filter.read()[message_type / 32] & (1 << (message_type % 32)) != 0
    }

    pub fn set_icmpv6_filter(&self, filter: [u32; 8]) -> AxResult {
        if self.family != IpFamily::Ipv6 || self.protocol != IpProtocol::Icmpv6 {
            return Err(AxError::from(LinuxError::ENOPROTOOPT));
        }
        *self.icmpv6_filter.write() = filter;
        Ok(())
    }

    pub fn set_ipv6_checksum_offset(&self, offset: i32) -> AxResult {
        if self.family != IpFamily::Ipv6 || self.protocol != IpProtocol::Icmpv6 || offset != 2 {
            return Err(AxError::InvalidInput);
        }
        Ok(())
    }

    pub fn set_ipv6_hop_limit_cmsg(&self, cmsg_type: u32, enabled: bool) -> AxResult {
        if self.family != IpFamily::Ipv6 {
            return Err(AxError::from(LinuxError::ENOPROTOOPT));
        }
        self.ipv6_hop_limit_cmsg
            .store(if enabled { cmsg_type } else { 0 }, Ordering::Relaxed);
        Ok(())
    }
}

impl Configurable for RawSocket {
    fn get_option_inner(&self, option: &mut GetSocketOption) -> AxResult<bool> {
        if self.general.get_option_inner(option)? {
            return Ok(true);
        }
        match option {
            GetSocketOption::Ttl(ttl) => **ttl = self.hop_limit.load(Ordering::Relaxed),
            GetSocketOption::Ipv6Only(value) if self.family == IpFamily::Ipv6 => **value = true,
            GetSocketOption::SendBuffer(size) | GetSocketOption::SendBufferForce(size) => {
                **size = self.with_socket(|socket| socket.payload_send_capacity());
            }
            GetSocketOption::ReceiveBuffer(size) | GetSocketOption::ReceiveBufferForce(size) => {
                **size = self.with_socket(|socket| socket.payload_recv_capacity());
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn set_option_inner(&self, option: SetSocketOption) -> AxResult<bool> {
        if self.general.set_option_inner(option)? {
            return Ok(true);
        }
        match option {
            SetSocketOption::Ttl(ttl) if *ttl != 0 => {
                self.hop_limit.store(*ttl, Ordering::Relaxed);
            }
            SetSocketOption::Ipv6Only(value) if self.family == IpFamily::Ipv6 && *value => {}
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl SocketOps for RawSocket {
    fn bind(&self, local_addr: SocketAddrEx) -> AxResult {
        let address = self.endpoint_address(local_addr.into_ip()?)?;
        if !SERVICE.lock().can_bind_ip(&address) {
            return Err(AxError::from(LinuxError::EADDRNOTAVAIL));
        }
        let mut local = self.local_addr.write();
        if local.is_some() {
            return Err(AxError::InvalidInput);
        }
        *local = Some(address);
        self.general
            .set_device_mask(SERVICE.lock().device_mask_for(&IpListenEndpoint {
                addr: (!address.is_unspecified()).then_some(address),
                port: 0,
            }));
        Ok(())
    }

    fn connect(&self, remote_addr: SocketAddrEx) -> AxResult {
        let address = self.endpoint_address(remote_addr.into_ip()?)?;
        if address.is_unspecified() {
            return Err(AxError::InvalidInput);
        }
        SERVICE.lock().get_source_address(&address)?;
        *self.peer_addr.write() = Some(address);
        self.general
            .set_device_mask(SERVICE.lock().device_mask_for(&IpListenEndpoint {
                addr: Some(address),
                port: 0,
            }));
        Ok(())
    }

    fn send(&self, mut src: impl Read + IoBuf, options: SendOptions) -> AxResult<usize> {
        let destination = match options.to {
            Some(address) => self.endpoint_address(address.into_ip()?)?,
            None => self.peer_addr.read().ok_or(AxError::NotConnected)?,
        };
        if destination.is_unspecified() {
            return Err(AxError::InvalidInput);
        }

        let mut payload = vec![0; src.remaining()];
        let payload_len = src.read(&mut payload)?;
        payload.truncate(payload_len);
        let packet = self.prepare_packet(&payload, destination)?;
        self.general
            .set_device_mask(SERVICE.lock().device_mask_for(&IpListenEndpoint {
                addr: Some(destination),
                port: 0,
            }));
        self.general.network_send_poller(self, || {
            self.with_socket(|socket| {
                if !socket.can_send() {
                    return Err(AxError::WouldBlock);
                }
                socket
                    .send_slice(&packet)
                    .map_err(|_| AxError::WouldBlock)?;
                Ok(())
            })
        })?;
        poll_interfaces();
        Ok(payload_len)
    }

    fn recv(
        &self,
        mut dst: impl Write + IoBufMut,
        mut options: RecvOptions<'_>,
    ) -> AxResult<usize> {
        let flags = options.flags;
        self.general.network_recv_poller(self, || {
            self.with_socket(|socket| {
                loop {
                    if !socket.can_recv() {
                        return Err(AxError::WouldBlock);
                    }

                    let parsed = {
                        let packet = socket.peek().map_err(|_| AxError::WouldBlock)?;
                        self.parse_received(packet)?
                    };
                    let (source, hop_limit, payload) = parsed;
                    if self.icmpv6_filtered(payload) {
                        socket.recv().map_err(|_| AxError::WouldBlock)?;
                        continue;
                    }

                    if let Some(from) = options.from.as_mut() {
                        **from = self.format_address(source);
                    }
                    if let Some(cmsg) = options.cmsg.as_mut() {
                        let cmsg_type = self.ipv6_hop_limit_cmsg.load(Ordering::Relaxed);
                        if cmsg_type != 0 && self.family == IpFamily::Ipv6 {
                            cmsg.push(Box::new(RecvControlMessage::Ipv6HopLimit {
                                value: i32::from(hop_limit),
                                cmsg_type,
                            }));
                        }
                    }

                    let packet_len = payload.len();
                    let written = dst.write(payload)?;
                    if !flags.contains(RecvFlags::PEEK) {
                        socket.recv().map_err(|_| AxError::WouldBlock)?;
                    }
                    return Ok(if flags.contains(RecvFlags::TRUNCATE) {
                        packet_len
                    } else {
                        written
                    });
                }
            })
        })
    }

    fn local_addr(&self) -> AxResult<SocketAddrEx> {
        Ok(self.format_address(
            self.local_addr
                .read()
                .unwrap_or_else(|| self.family.unspecified_addr()),
        ))
    }

    fn peer_addr(&self) -> AxResult<SocketAddrEx> {
        self.peer_addr
            .read()
            .map(|address| self.format_address(address))
            .ok_or(AxError::NotConnected)
    }

    fn shutdown(&self, _how: Shutdown) -> AxResult {
        Ok(())
    }
}

impl Pollable for RawSocket {
    fn poll(&self) -> IoEvents {
        self.with_socket(|socket| {
            let mut events = IoEvents::empty();
            events.set(IoEvents::IN, socket.can_recv());
            events.set(IoEvents::OUT, socket.can_send());
            events
        })
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.intersects(IoEvents::IN | IoEvents::OUT) {
            self.general.register_waker(context.waker());
        }
    }
}

impl Drop for RawSocket {
    fn drop(&mut self) {
        SOCKET_SET.remove(self.handle);
    }
}
