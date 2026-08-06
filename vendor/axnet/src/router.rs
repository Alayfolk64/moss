use alloc::{boxed::Box, vec, vec::Vec};

use smoltcp::{
    iface::SocketSet,
    phy::{DeviceCapabilities, Medium},
    storage::PacketMetadata,
    time::Instant,
    wire::{IpAddress, IpCidr, IpProtocol, IpVersion, Ipv4Packet, Ipv6Packet, TcpPacket},
};

use crate::{
    LISTEN_TABLE,
    consts::{LOOPBACK_MTU, ROUTER_PACKET_BUFFER_SIZE, STANDARD_MTU},
    device::Device,
};

#[derive(Debug)]
pub struct Rule {
    pub filter: IpCidr,
    pub via: Option<IpAddress>,
    pub dev: usize,
    pub src: IpAddress,
}

impl Rule {
    pub fn new(filter: IpCidr, via: Option<IpAddress>, dev: usize, src: IpAddress) -> Self {
        Self {
            filter,
            via,
            dev,
            src,
        }
    }
}

type PacketBuffer = smoltcp::storage::PacketBuffer<'static, ()>;

// TODO(mivik): optimize
pub struct RouteTable {
    rules: Vec<Rule>,
}
impl RouteTable {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn add_rule(&mut self, rule: Rule) {
        let idx = self
            .rules
            .binary_search_by(|it| rule.filter.prefix_len().cmp(&it.filter.prefix_len()))
            .unwrap_or_else(|idx| idx);
        self.rules.insert(idx, rule);
    }

    pub fn lookup(&self, dst: &IpAddress) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|rule| rule.filter.contains_addr(dst))
    }
}

pub struct Router {
    rx_buffer: PacketBuffer,
    tx_buffer: PacketBuffer,
    poll_next: bool,
    pub(crate) devices: Vec<Box<dyn Device>>,
    pub(crate) table: RouteTable,
}
impl Router {
    pub fn new() -> Self {
        let rx_buffer = PacketBuffer::new(
            vec![PacketMetadata::EMPTY; ROUTER_PACKET_BUFFER_SIZE],
            vec![0u8; LOOPBACK_MTU * ROUTER_PACKET_BUFFER_SIZE],
        );
        let tx_buffer = PacketBuffer::new(
            vec![PacketMetadata::EMPTY; ROUTER_PACKET_BUFFER_SIZE],
            vec![0u8; LOOPBACK_MTU * ROUTER_PACKET_BUFFER_SIZE],
        );
        Self {
            rx_buffer,
            tx_buffer,
            poll_next: false,
            devices: Vec::new(),
            table: RouteTable::new(),
        }
    }

    pub fn add_rule(&mut self, rule: Rule) {
        self.table.add_rule(rule);
    }

    pub fn add_device(&mut self, device: Box<dyn Device>) -> usize {
        self.devices.push(device);
        self.devices.len() - 1
    }

    pub fn poll(&mut self, timestamp: Instant) {
        for dev in &mut self.devices {
            while !self.rx_buffer.is_full() && dev.recv(&mut self.rx_buffer, timestamp) {}
            self.poll_next |= dev.poll_complete();
        }
    }

    fn enqueue_rx(rx_buffer: &mut PacketBuffer, packet: &[u8]) -> bool {
        match rx_buffer.enqueue(packet.len(), ()) {
            Ok(buf) => {
                buf.copy_from_slice(packet);
                true
            }
            Err(_) => false,
        }
    }

    fn send_via_rule(
        devices: &mut [Box<dyn Device>],
        rx_buffer: &mut PacketBuffer,
        dst_addr: IpAddress,
        rule_dev: usize,
        packet: &[u8],
        timestamp: Instant,
    ) -> bool {
        if devices[rule_dev].is_loopback() {
            if Self::enqueue_rx(rx_buffer, packet) {
                devices[rule_dev].notify_rx_ready();
                true
            } else {
                devices[rule_dev].send(dst_addr, packet, timestamp)
            }
        } else {
            let dev = &mut devices[rule_dev];
            dev.send(dst_addr, packet, timestamp)
        }
    }

    fn packet_route(table: &RouteTable, packet: &[u8]) -> PacketRoute {
        match IpVersion::of_packet(packet).expect("got invalid IP packet") {
            IpVersion::Ipv4 => {
                let packet =
                    smoltcp::wire::Ipv4Packet::new_checked(packet).expect("got invalid IPv4 packet");
                let dst_addr = IpAddress::Ipv4(packet.dst_addr());
                if packet.dst_addr().is_broadcast() {
                    PacketRoute::Broadcast(dst_addr)
                } else {
                    let Some(rule) = table.lookup(&dst_addr) else {
                        warn!("No route found for destination: {}", dst_addr);
                        return PacketRoute::Drop;
                    };
                    if rule.src != IpAddress::Ipv4(packet.src_addr()) {
                        warn!("Invalid source address {} for route to {}", packet.src_addr(), dst_addr);
                        return PacketRoute::Drop;
                    }
                    PacketRoute::Unicast {
                        next_hop: rule.via.unwrap_or(dst_addr),
                        dev: rule.dev,
                    }
                }
            }
            IpVersion::Ipv6 => {
                let packet =
                    smoltcp::wire::Ipv6Packet::new_checked(packet).expect("got invalid IPv6 packet");
                let dst_addr = IpAddress::Ipv6(packet.dst_addr());
                if packet.dst_addr().is_multicast() {
                    PacketRoute::Multicast(dst_addr)
                } else {
                    let Some(rule) = table.lookup(&dst_addr) else {
                        warn!("No route found for destination: {}", dst_addr);
                        return PacketRoute::Drop;
                    };
                    if rule.src != IpAddress::Ipv6(packet.src_addr()) {
                        warn!("Invalid source address {} for route to {}", packet.src_addr(), dst_addr);
                        return PacketRoute::Drop;
                    }
                    PacketRoute::Unicast {
                        next_hop: rule.via.unwrap_or(dst_addr),
                        dev: rule.dev,
                    }
                }
            }
        }
    }

    fn route_packet(
        devices: &mut [Box<dyn Device>],
        rx_buffer: &mut PacketBuffer,
        route: PacketRoute,
        packet: &[u8],
        timestamp: Instant,
    ) -> bool {
        match route {
            PacketRoute::Broadcast(dst_addr) | PacketRoute::Multicast(dst_addr) => {
                let mut poll_next = false;
                for dev in devices.iter_mut() {
                    poll_next |= dev.send(dst_addr, packet, timestamp);
                }
                poll_next
            }
            PacketRoute::Unicast {
                next_hop, dev, ..
            } => Self::send_via_rule(devices, rx_buffer, next_hop, dev, packet, timestamp),
            PacketRoute::Drop => false,
        }
    }

    pub fn dispatch(&mut self, timestamp: Instant) -> bool {
        let Self {
            rx_buffer,
            tx_buffer,
            poll_next,
            devices,
            table,
        } = self;
        let mut next = core::mem::take(poll_next);
        while let Ok(((), packet)) = tx_buffer.dequeue() {
            let route = Self::packet_route(table, packet);
            next |= Self::route_packet(devices, rx_buffer, route, packet, timestamp);
        }
        next
    }
}

#[derive(Clone, Copy)]
enum PacketRoute {
    Broadcast(IpAddress),
    Multicast(IpAddress),
    Unicast {
        next_hop: IpAddress,
        dev: usize,
    },
    Drop,
}

pub enum TxToken<'a> {
    Buffered {
        tx_buffer: &'a mut PacketBuffer,
    },
    Drop,
    Routing {
        rx_buffer: &'a mut PacketBuffer,
        tx_buffer: &'a mut PacketBuffer,
        poll_next: &'a mut bool,
        devices: &'a mut [Box<dyn Device>],
        table: &'a RouteTable,
        dst_addr: Option<IpAddress>,
    },
}

impl TxToken<'_> {
    fn buffered_consume<R, F>(tx_buffer: &mut PacketBuffer, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        match tx_buffer.enqueue(len, ()) {
            Ok(buf) => f(buf),
            Err(_) => {
                let mut dropped = vec![0; len];
                f(&mut dropped)
            }
        }
    }

    fn try_direct_loopback_consume<R, F>(
        rx_buffer: &mut PacketBuffer,
        poll_next: &mut bool,
        devices: &[Box<dyn Device>],
        dev: usize,
        len: usize,
        emit: &mut Option<F>,
    ) -> Option<R>
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        match rx_buffer.enqueue(len, ()) {
            Ok(buf) => {
                let result = emit.take().unwrap()(buf);
                devices[dev].notify_rx_ready();
                *poll_next = true;
                Some(result)
            }
            Err(_) => None,
        }
    }

    fn loopback_route_dev(
        devices: &[Box<dyn Device>],
        table: &RouteTable,
        dst_addr: Option<IpAddress>,
    ) -> Option<usize> {
        let Some(addr) = dst_addr else {
            return None;
        };
        table.lookup(&addr).and_then(|rule| {
            (rule.via.is_none() && devices[rule.dev].is_loopback()).then_some(rule.dev)
        })
    }

    fn fallback_packet(tx_buffer: &mut PacketBuffer, packet: &[u8]) {
        if let Ok(buf) = tx_buffer.enqueue(packet.len(), ()) {
            buf.copy_from_slice(packet);
        }
    }
}

impl smoltcp::phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        match self {
            TxToken::Buffered { tx_buffer } => Self::buffered_consume(tx_buffer, len, f),
            TxToken::Drop => {
                let mut dropped = vec![0; len];
                f(&mut dropped)
            }
            TxToken::Routing {
                rx_buffer,
                tx_buffer,
                poll_next,
                devices,
                table,
                dst_addr,
            } => {
                if let Some(dev) = Self::loopback_route_dev(devices, table, dst_addr) {
                    let mut emit = Some(f);
                    if let Some(result) = Self::try_direct_loopback_consume(
                        rx_buffer,
                        poll_next,
                        devices,
                        dev,
                        len,
                        &mut emit,
                    ) {
                        return result;
                    }

                    let mut packet = vec![0; len];
                    let result = emit.take().unwrap()(&mut packet);
                    Self::fallback_packet(tx_buffer, &packet);
                    return result;
                }

                Self::buffered_consume(tx_buffer, len, f)
            }
        }
    }

    fn set_dst_addr(&mut self, dst_addr: IpAddress) {
        if let TxToken::Routing { dst_addr: hint, .. } = self {
            *hint = Some(dst_addr);
        }
    }
}

fn snoop_tcp_packet(buf: &[u8], sockets: &mut SocketSet<'_>) {
    let (protocol, src_addr, dst_addr, payload) = match IpVersion::of_packet(buf).unwrap() {
        IpVersion::Ipv4 => {
            let packet = Ipv4Packet::new_unchecked(buf);
            (
                packet.next_header(),
                IpAddress::Ipv4(packet.src_addr()),
                IpAddress::Ipv4(packet.dst_addr()),
                packet.payload(),
            )
        }
        IpVersion::Ipv6 => {
            let packet = Ipv6Packet::new_unchecked(buf);
            (
                packet.next_header(),
                IpAddress::Ipv6(packet.src_addr()),
                IpAddress::Ipv6(packet.dst_addr()),
                packet.payload(),
            )
        }
    };
    if protocol == IpProtocol::Tcp {
        let tcp_packet = TcpPacket::new_unchecked(payload);
        let src_addr = (src_addr, tcp_packet.src_port()).into();
        let dst_addr = (dst_addr, tcp_packet.dst_port()).into();
        let is_first = tcp_packet.syn() && !tcp_packet.ack();
        if is_first {
            LISTEN_TABLE.incoming_tcp_packet(src_addr, dst_addr, sockets);
        }
    }
}

pub struct RxToken<'a>(&'a [u8]);

impl<'a> smoltcp::phy::RxToken for RxToken<'a> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.0)
    }

    fn preprocess(&self, sockets: &mut SocketSet) {
        snoop_tcp_packet(self.0, sockets);
    }
}

impl smoltcp::phy::Device for Router {
    type RxToken<'a> = RxToken<'a>;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if self.rx_buffer.is_empty() {
            None
        } else if self.tx_buffer.is_full() {
            Some((RxToken(self.rx_buffer.dequeue().unwrap().1), TxToken::Drop))
        } else {
            Some((
                RxToken(self.rx_buffer.dequeue().unwrap().1),
                TxToken::Buffered {
                    tx_buffer: &mut self.tx_buffer,
                },
            ))
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.tx_buffer.is_full() {
            None
        } else {
            let Self {
                rx_buffer,
                tx_buffer,
                poll_next,
                devices,
                table,
            } = self;
            Some(TxToken::Routing {
                rx_buffer,
                tx_buffer,
                poll_next,
                devices,
                table,
                dst_addr: None,
            })
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = STANDARD_MTU;
        caps.loopback_transmission_unit = Some(LOOPBACK_MTU);
        caps.max_burst_size = Some(ROUTER_PACKET_BUFFER_SIZE);
        caps
    }
}
