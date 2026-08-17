use alloc::{string::String, vec, vec::Vec};
use core::task::Waker;

use axdriver::prelude::*;
use axpoll::PollSet;
use axtask::future::register_irq_waker;
use hashbrown::HashMap;
use smoltcp::{
    phy::ChecksumCapabilities,
    storage::{PacketBuffer, PacketMetadata},
    time::{Duration, Instant},
    wire::{
        ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
        EthernetRepr, Icmpv6Packet, Icmpv6Repr, IpAddress, IpCidr, IpProtocol, Ipv4Cidr,
        Ipv4Packet, Ipv6Address, Ipv6Cidr, Ipv6Packet, Ipv6Repr, NdiscNeighborFlags,
        NdiscPrefixInfoFlags, NdiscRepr,
    },
};

use crate::{
    consts::{ETHERNET_MAX_PENDING_PACKETS, STANDARD_MTU},
    device::{Device, Ipv6AutoconfigEvent, NeighborEntry, NeighborState},
};

const EMPTY_MAC: EthernetAddress = EthernetAddress([0; 6]);

enum Neighbor {
    Incomplete {
        retry_at: Instant,
    },
    Reachable {
        hardware_address: EthernetAddress,
        expires_at: Instant,
    },
    Permanent {
        hardware_address: EthernetAddress,
    },
}

pub struct EthernetDevice {
    name: String,
    inner: AxNetDevice,
    neighbors: HashMap<IpAddress, Neighbor>,
    ipv4_addrs: Vec<Ipv4Cidr>,
    ipv6_addrs: Vec<Ipv6Cidr>,
    next_router_solicit: Instant,
    router_solicits_remaining: u8,
    ipv6_autoconfig_event: Option<Ipv6AutoconfigEvent>,
    rx_wakers: PollSet,

    pending_packets: PacketBuffer<'static, IpAddress>,
}
impl EthernetDevice {
    const NEIGHBOR_TTL: Duration = Duration::from_secs(60);
    const NEIGHBOR_RETRY: Duration = Duration::from_secs(1);
    const ROUTER_SOLICIT_INTERVAL: Duration = Duration::from_secs(4);
    const MAX_ROUTER_SOLICITATIONS: u8 = 3;

    pub fn new(name: String, inner: AxNetDevice, addrs: &[IpCidr]) -> Self {
        let pending_packets = PacketBuffer::new(
            vec![PacketMetadata::EMPTY; ETHERNET_MAX_PENDING_PACKETS],
            vec![
                0u8;
                (STANDARD_MTU + EthernetFrame::<&[u8]>::header_len())
                    * ETHERNET_MAX_PENDING_PACKETS
            ],
        );
        let mut device = Self {
            name,
            inner,
            neighbors: HashMap::new(),
            ipv4_addrs: Vec::new(),
            ipv6_addrs: Vec::new(),
            next_router_solicit: Instant::from_millis(0),
            router_solicits_remaining: Self::MAX_ROUTER_SOLICITATIONS,
            ipv6_autoconfig_event: None,
            rx_wakers: PollSet::new(),

            pending_packets,
        };
        device.set_ip_addrs(addrs);
        device
    }

    fn set_ip_addrs(&mut self, addrs: &[IpCidr]) {
        self.ipv4_addrs.clear();
        self.ipv6_addrs.clear();
        for addr in addrs {
            match *addr {
                IpCidr::Ipv4(addr) => self.ipv4_addrs.push(addr),
                IpCidr::Ipv6(addr) => self.ipv6_addrs.push(addr),
            }
        }
        self.neighbors.clear();
        while self.pending_packets.dequeue().is_ok() {}
    }

    #[inline]
    fn hardware_address(&self) -> EthernetAddress {
        EthernetAddress(self.inner.mac_address().0)
    }

    fn send_to<F>(
        inner: &mut AxNetDevice,
        dst: EthernetAddress,
        size: usize,
        f: F,
        proto: EthernetProtocol,
    ) where
        F: FnOnce(&mut [u8]),
    {
        if let Err(err) = inner.recycle_tx_buffers() {
            if !matches!(err, DevError::Again) {
                warn!("recycle_tx_buffers failed: {:?}", err);
            }
            return;
        }

        let repr = EthernetRepr {
            src_addr: EthernetAddress(inner.mac_address().0),
            dst_addr: dst,
            ethertype: proto,
        };

        let mut tx_buf = match inner.alloc_tx_buffer(repr.buffer_len() + size) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("alloc_tx_buffer failed: {:?}", err);
                return;
            }
        };
        let mut frame = EthernetFrame::new_unchecked(tx_buf.packet_mut());
        repr.emit(&mut frame);
        f(frame.payload_mut());
        trace!(
            "SEND {} bytes: {:02X?}",
            tx_buf.packet_len(),
            tx_buf.packet()
        );
        if let Err(err) = inner.transmit(tx_buf) {
            warn!("transmit failed: {:?}", err);
        }
    }

    fn handle_frame(
        &mut self,
        frame: &[u8],
        buffer: &mut PacketBuffer<()>,
        timestamp: Instant,
    ) -> bool {
        let frame = EthernetFrame::new_unchecked(frame);
        let Ok(repr) = EthernetRepr::parse(&frame) else {
            warn!("Dropping malformed Ethernet frame");
            return false;
        };

        if !repr.dst_addr.is_broadcast()
            && !repr.dst_addr.is_multicast()
            && repr.dst_addr != EMPTY_MAC
            && repr.dst_addr != self.hardware_address()
        {
            return false;
        }

        match repr.ethertype {
            EthernetProtocol::Ipv4 => {
                self.learn_ip_neighbor(
                    Ipv4Packet::new_checked(frame.payload())
                        .ok()
                        .map(|packet| IpAddress::Ipv4(packet.src_addr())),
                    repr.src_addr,
                    timestamp,
                );
                return match buffer.enqueue(frame.payload().len(), ()) {
                    Ok(buf) => {
                        buf.copy_from_slice(frame.payload());
                        true
                    }
                    Err(_) => false,
                };
            }
            EthernetProtocol::Arp => self.process_arp(frame.payload(), timestamp),
            EthernetProtocol::Ipv6 => {
                if self.process_ndp(frame.payload(), repr.src_addr, timestamp) {
                    return false;
                }
                self.learn_ip_neighbor(
                    Ipv6Packet::new_checked(frame.payload())
                        .ok()
                        .map(|packet| IpAddress::Ipv6(packet.src_addr())),
                    repr.src_addr,
                    timestamp,
                );
                return match buffer.enqueue(frame.payload().len(), ()) {
                    Ok(buf) => {
                        buf.copy_from_slice(frame.payload());
                        true
                    }
                    Err(_) => false,
                };
            }
            _ => {}
        }

        false
    }

    fn learn_ip_neighbor(
        &mut self,
        address: Option<IpAddress>,
        hardware_address: EthernetAddress,
        now: Instant,
    ) {
        let Some(address) = address.filter(|address| !address.is_unspecified()) else {
            return;
        };
        let on_link = match address {
            IpAddress::Ipv4(address) => self
                .ipv4_addrs
                .iter()
                .any(|configured| configured.contains_addr(&address)),
            IpAddress::Ipv6(address) => self
                .ipv6_addrs
                .iter()
                .any(|configured| configured.contains_addr(&address)),
        };
        if !on_link {
            return;
        }
        if !hardware_address.is_unicast() {
            return;
        }
        self.neighbors.insert(
            address,
            Neighbor::Reachable {
                hardware_address,
                expires_at: now + Self::NEIGHBOR_TTL,
            },
        );
        self.flush_pending(address, now);
    }

    fn request_arp(&mut self, target_ipv4: smoltcp::wire::Ipv4Address, now: Instant) {
        let Some(source_ip) = self.ipv4_addrs.first().map(Ipv4Cidr::address) else {
            warn!(
                "Cannot request ARP for {} without an IPv4 address",
                target_ipv4
            );
            return;
        };
        debug!("Requesting ARP for {}", target_ipv4);

        let arp_repr = ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Request,
            source_hardware_addr: self.hardware_address(),
            source_protocol_addr: source_ip,
            target_hardware_addr: EMPTY_MAC,
            target_protocol_addr: target_ipv4,
        };

        Self::send_to(
            &mut self.inner,
            EthernetAddress::BROADCAST,
            arp_repr.buffer_len(),
            |buf| arp_repr.emit(&mut ArpPacket::new_unchecked(buf)),
            EthernetProtocol::Arp,
        );

        self.neighbors.insert(
            IpAddress::Ipv4(target_ipv4),
            Neighbor::Incomplete {
                retry_at: now + Self::NEIGHBOR_RETRY,
            },
        );
    }

    fn process_arp(&mut self, payload: &[u8], now: Instant) {
        let Ok(repr) = ArpPacket::new_checked(payload).and_then(|packet| ArpRepr::parse(&packet))
        else {
            warn!("Dropping malformed ARP packet");
            return;
        };

        if let ArpRepr::EthernetIpv4 {
            operation,
            source_hardware_addr,
            source_protocol_addr,
            target_hardware_addr,
            target_protocol_addr,
        } = repr
        {
            let is_unicast_mac =
                target_hardware_addr != EMPTY_MAC && !target_hardware_addr.is_broadcast();
            if is_unicast_mac && self.hardware_address() != target_hardware_addr {
                // Only process packet that are for us
                return;
            }

            if let ArpOperation::Unknown(_) = operation {
                return;
            }

            if !source_hardware_addr.is_unicast()
                || source_protocol_addr.is_broadcast()
                || source_protocol_addr.is_multicast()
                || source_protocol_addr.is_unspecified()
            {
                return;
            }
            if !self
                .ipv4_addrs
                .iter()
                .any(|address| address.address() == target_protocol_addr)
            {
                return;
            }

            debug!("ARP: {} -> {}", source_protocol_addr, source_hardware_addr);
            self.neighbors.insert(
                IpAddress::Ipv4(source_protocol_addr),
                Neighbor::Reachable {
                    hardware_address: source_hardware_addr,
                    expires_at: now + Self::NEIGHBOR_TTL,
                },
            );

            if let ArpOperation::Request = operation {
                let response = ArpRepr::EthernetIpv4 {
                    operation: ArpOperation::Reply,
                    source_hardware_addr: self.hardware_address(),
                    source_protocol_addr: target_protocol_addr,
                    target_hardware_addr: source_hardware_addr,
                    target_protocol_addr: source_protocol_addr,
                };

                Self::send_to(
                    &mut self.inner,
                    source_hardware_addr,
                    response.buffer_len(),
                    |buf| response.emit(&mut ArpPacket::new_unchecked(buf)),
                    EthernetProtocol::Arp,
                );
            }

            self.flush_pending(IpAddress::Ipv4(source_protocol_addr), now);
        }
    }

    fn ipv6_solicited_node(address: Ipv6Address) -> Ipv6Address {
        let bytes = address.octets();
        Ipv6Address::from([
            0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xff, bytes[13], bytes[14], bytes[15],
        ])
    }

    fn ipv4_multicast_mac(address: smoltcp::wire::Ipv4Address) -> EthernetAddress {
        let bytes = address.octets();
        EthernetAddress([0x01, 0x00, 0x5e, bytes[1] & 0x7f, bytes[2], bytes[3]])
    }

    fn ipv6_multicast_mac(address: Ipv6Address) -> EthernetAddress {
        let bytes = address.octets();
        EthernetAddress([0x33, 0x33, bytes[12], bytes[13], bytes[14], bytes[15]])
    }

    fn slaac_address(&self, prefix: Ipv6Address) -> Ipv6Address {
        let mac = self.hardware_address().0;
        let mut address = prefix.octets();
        address[8] = mac[0] ^ 0x02;
        address[9] = mac[1];
        address[10] = mac[2];
        address[11] = 0xff;
        address[12] = 0xfe;
        address[13] = mac[3];
        address[14] = mac[4];
        address[15] = mac[5];
        Ipv6Address::from(address)
    }

    fn send_ndp_packet(
        &mut self,
        source_ip: Ipv6Address,
        destination_ip: Ipv6Address,
        destination_mac: EthernetAddress,
        repr: NdiscRepr<'_>,
    ) {
        let icmp_repr = Icmpv6Repr::Ndisc(repr);
        let ipv6_repr = Ipv6Repr {
            src_addr: source_ip,
            dst_addr: destination_ip,
            next_header: IpProtocol::Icmpv6,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 255,
        };
        let packet_len = ipv6_repr.buffer_len() + icmp_repr.buffer_len();
        Self::send_to(
            &mut self.inner,
            destination_mac,
            packet_len,
            |buffer| {
                let mut ipv6_packet = Ipv6Packet::new_unchecked(buffer);
                ipv6_repr.emit(&mut ipv6_packet);
                icmp_repr.emit(
                    &source_ip,
                    &destination_ip,
                    &mut Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut()),
                    &ChecksumCapabilities::default(),
                );
            },
            EthernetProtocol::Ipv6,
        );
    }

    fn request_ndp(&mut self, target_ip: Ipv6Address, now: Instant) {
        let Some(source_ip) = self
            .ipv6_addrs
            .iter()
            .find(|address| address.address().is_unicast_link_local())
            .or_else(|| self.ipv6_addrs.first())
            .map(Ipv6Cidr::address)
        else {
            warn!(
                "Cannot request NDP for {} without an IPv6 address",
                target_ip
            );
            return;
        };
        let destination_ip = Self::ipv6_solicited_node(target_ip);
        debug!("Requesting NDP for {}", target_ip);
        self.send_ndp_packet(
            source_ip,
            destination_ip,
            Self::ipv6_multicast_mac(destination_ip),
            NdiscRepr::NeighborSolicit {
                target_addr: target_ip,
                lladdr: Some(self.hardware_address().into()),
            },
        );
        self.neighbors.insert(
            IpAddress::Ipv6(target_ip),
            Neighbor::Incomplete {
                retry_at: now + Self::NEIGHBOR_RETRY,
            },
        );
    }

    fn solicit_ipv6_router(&mut self, now: Instant) {
        if self.router_solicits_remaining == 0 || now < self.next_router_solicit {
            return;
        }
        let Some(source_ip) = self
            .ipv6_addrs
            .iter()
            .find(|address| address.address().is_unicast_link_local())
            .map(Ipv6Cidr::address)
        else {
            return;
        };
        let all_routers = Ipv6Address::new(0xff02, 0, 0, 0, 0, 0, 0, 2);
        self.send_ndp_packet(
            source_ip,
            all_routers,
            Self::ipv6_multicast_mac(all_routers),
            NdiscRepr::RouterSolicit {
                lladdr: Some(self.hardware_address().into()),
            },
        );
        self.router_solicits_remaining -= 1;
        self.next_router_solicit = now + Self::ROUTER_SOLICIT_INTERVAL;
    }

    fn process_ndp(&mut self, payload: &[u8], source_mac: EthernetAddress, now: Instant) -> bool {
        let Ok(ipv6_packet) = Ipv6Packet::new_checked(payload) else {
            return false;
        };
        if ipv6_packet.next_header() != IpProtocol::Icmpv6 || ipv6_packet.hop_limit() != 255 {
            return false;
        }
        let source_ip = ipv6_packet.src_addr();
        let destination_ip = ipv6_packet.dst_addr();
        let Ok(icmp_packet) = Icmpv6Packet::new_checked(ipv6_packet.payload()) else {
            return false;
        };
        let Ok(Icmpv6Repr::Ndisc(repr)) = Icmpv6Repr::parse(
            &source_ip,
            &destination_ip,
            &icmp_packet,
            &ChecksumCapabilities::default(),
        ) else {
            return false;
        };

        match repr {
            NdiscRepr::NeighborAdvert {
                target_addr,
                lladdr,
                ..
            } => {
                let hardware_address = lladdr
                    .filter(|address| address.len() == 6)
                    .map(|address| EthernetAddress::from_bytes(address.as_bytes()))
                    .unwrap_or(source_mac);
                self.learn_ip_neighbor(Some(IpAddress::Ipv6(target_addr)), hardware_address, now);
            }
            NdiscRepr::NeighborSolicit {
                target_addr,
                lladdr,
            } => {
                let hardware_address = lladdr
                    .filter(|address| address.len() == 6)
                    .map(|address| EthernetAddress::from_bytes(address.as_bytes()))
                    .unwrap_or(source_mac);
                if !source_ip.is_unspecified() {
                    self.learn_ip_neighbor(Some(IpAddress::Ipv6(source_ip)), hardware_address, now);
                }
                if self
                    .ipv6_addrs
                    .iter()
                    .any(|address| address.address() == target_addr)
                {
                    let (reply_ip, reply_mac, flags) = if source_ip.is_unspecified() {
                        let all_nodes = Ipv6Address::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
                        (
                            all_nodes,
                            Self::ipv6_multicast_mac(all_nodes),
                            NdiscNeighborFlags::OVERRIDE,
                        )
                    } else {
                        (
                            source_ip,
                            hardware_address,
                            NdiscNeighborFlags::SOLICITED | NdiscNeighborFlags::OVERRIDE,
                        )
                    };
                    self.send_ndp_packet(
                        target_addr,
                        reply_ip,
                        reply_mac,
                        NdiscRepr::NeighborAdvert {
                            flags,
                            target_addr,
                            lladdr: Some(self.hardware_address().into()),
                        },
                    );
                }
            }
            NdiscRepr::RouterAdvert {
                router_lifetime,
                lladdr,
                prefix_info,
                ..
            } => {
                self.router_solicits_remaining = 0;
                let hardware_address = lladdr
                    .filter(|address| address.len() == 6)
                    .map(|address| EthernetAddress::from_bytes(address.as_bytes()))
                    .unwrap_or(source_mac);
                self.learn_ip_neighbor(Some(IpAddress::Ipv6(source_ip)), hardware_address, now);
                let address = prefix_info.and_then(|prefix| {
                    (prefix.prefix_len == 64
                        && prefix.flags.contains(NdiscPrefixInfoFlags::ADDRCONF)
                        && prefix.valid_lifetime > Duration::ZERO
                        && !prefix.prefix.is_multicast()
                        && !prefix.prefix.is_unicast_link_local())
                    .then(|| Ipv6Cidr::new(self.slaac_address(prefix.prefix), 64))
                });
                self.ipv6_autoconfig_event = Some(Ipv6AutoconfigEvent {
                    address,
                    router: (router_lifetime > Duration::ZERO && source_ip.is_unicast_link_local())
                        .then_some(source_ip),
                });
            }
            _ => {}
        }
        true
    }

    fn send_resolved_packet(
        &mut self,
        next_hop: IpAddress,
        hardware_address: EthernetAddress,
        packet: &[u8],
    ) {
        let protocol = match next_hop {
            IpAddress::Ipv4(_) => EthernetProtocol::Ipv4,
            IpAddress::Ipv6(_) => EthernetProtocol::Ipv6,
        };
        Self::send_to(
            &mut self.inner,
            hardware_address,
            packet.len(),
            |buffer| buffer.copy_from_slice(packet),
            protocol,
        );
    }

    fn flush_pending(&mut self, address: IpAddress, now: Instant) {
        while self
            .pending_packets
            .peek()
            .is_ok_and(|(next_hop, _)| *next_hop == address)
        {
            let hardware_address = match self.neighbors.get(&address) {
                Some(Neighbor::Reachable {
                    hardware_address,
                    expires_at,
                }) if *expires_at > now => *hardware_address,
                _ => break,
            };
            let protocol = match address {
                IpAddress::Ipv4(_) => EthernetProtocol::Ipv4,
                IpAddress::Ipv6(_) => EthernetProtocol::Ipv6,
            };
            let (_, packet) = self.pending_packets.dequeue().unwrap();
            Self::send_to(
                &mut self.inner,
                hardware_address,
                packet.len(),
                |buffer| buffer.copy_from_slice(packet),
                protocol,
            );
        }
    }
}

impl Device for EthernetDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn notify_rx_ready(&self) {
        self.rx_wakers.wake();
    }

    fn update_ip_addrs(&mut self, addrs: &[IpCidr]) {
        self.set_ip_addrs(addrs);
    }

    fn neighbor_entries(&self, timestamp: Instant, entries: &mut Vec<NeighborEntry>) {
        entries.extend(
            self.neighbors
                .iter()
                .map(|(address, neighbor)| match neighbor {
                    Neighbor::Incomplete { .. } => NeighborEntry {
                        address: *address,
                        hardware_address: None,
                        state: NeighborState::Incomplete,
                    },
                    Neighbor::Reachable {
                        hardware_address,
                        expires_at,
                    } => NeighborEntry {
                        address: *address,
                        hardware_address: Some(hardware_address.0),
                        state: if *expires_at > timestamp {
                            NeighborState::Reachable
                        } else {
                            NeighborState::Stale
                        },
                    },
                    Neighbor::Permanent { hardware_address } => NeighborEntry {
                        address: *address,
                        hardware_address: Some(hardware_address.0),
                        state: NeighborState::Permanent,
                    },
                }),
        );
    }

    fn remove_neighbor(&mut self, address: IpAddress) -> bool {
        self.neighbors.remove(&address).is_some()
    }

    fn set_neighbor(&mut self, address: IpAddress, hardware_address: [u8; 6]) -> bool {
        let hardware_address = EthernetAddress(hardware_address);
        if address.is_unspecified()
            || address.is_multicast()
            || address.is_broadcast()
            || !hardware_address.is_unicast()
        {
            return false;
        }
        self.neighbors
            .insert(address, Neighbor::Permanent { hardware_address });
        true
    }

    fn take_ipv6_autoconfig_event(&mut self) -> Option<Ipv6AutoconfigEvent> {
        self.ipv6_autoconfig_event.take()
    }

    fn recv(&mut self, buffer: &mut PacketBuffer<()>, timestamp: Instant) -> bool {
        self.solicit_ipv6_router(timestamp);
        // Broadcast storms and malformed traffic must not keep SERVICE locked
        // forever while this loop discards frames that do not reach smoltcp.
        // Deferred device polling will revisit a still-ready RX descriptor.
        const MAX_RX_FRAMES_PER_POLL: usize = 64;
        for _ in 0..MAX_RX_FRAMES_PER_POLL {
            let rx_buf = match self.inner.receive() {
                Ok(buf) => buf,
                Err(err) => {
                    if !matches!(err, DevError::Again) {
                        warn!("receive failed: {:?}", err);
                    }
                    return false;
                }
            };
            trace!(
                "RECV {} bytes: {:02X?}",
                rx_buf.packet_len(),
                rx_buf.packet()
            );

            let result = self.handle_frame(rx_buf.packet(), buffer, timestamp);
            self.inner.recycle_rx_buffer(rx_buf).unwrap();
            if result {
                self.notify_rx_ready();
                return true;
            }
        }
        false
    }

    fn send(&mut self, next_hop: IpAddress, packet: &[u8], timestamp: Instant) -> bool {
        if packet.len() > STANDARD_MTU {
            warn!(
                "Dropping oversized Ethernet packet to {}: {} > {}",
                next_hop,
                packet.len(),
                STANDARD_MTU
            );
            return false;
        }

        match next_hop {
            IpAddress::Ipv4(address) if address == smoltcp::wire::Ipv4Address::BROADCAST => {
                self.send_resolved_packet(next_hop, EthernetAddress::BROADCAST, packet);
                return false;
            }
            IpAddress::Ipv4(address) if address.is_multicast() => {
                self.send_resolved_packet(next_hop, Self::ipv4_multicast_mac(address), packet);
                return false;
            }
            IpAddress::Ipv4(address)
                if self
                    .ipv4_addrs
                    .iter()
                    .any(|local| local.broadcast() == Some(address)) =>
            {
                self.send_resolved_packet(next_hop, EthernetAddress::BROADCAST, packet);
                return false;
            }
            IpAddress::Ipv6(address) if address.is_multicast() => {
                self.send_resolved_packet(next_hop, Self::ipv6_multicast_mac(address), packet);
                return false;
            }
            _ => {}
        }

        let need_request = match self.neighbors.get(&next_hop) {
            Some(Neighbor::Permanent { hardware_address }) => {
                let hardware_address = *hardware_address;
                self.send_resolved_packet(next_hop, hardware_address, packet);
                return false;
            }
            Some(Neighbor::Reachable {
                hardware_address,
                expires_at,
            }) if *expires_at > timestamp => {
                let hardware_address = *hardware_address;
                self.send_resolved_packet(next_hop, hardware_address, packet);
                return false;
            }
            Some(Neighbor::Incomplete { retry_at }) => *retry_at <= timestamp,
            Some(Neighbor::Reachable { .. }) | None => true,
        };
        if need_request {
            match next_hop {
                IpAddress::Ipv4(address) => self.request_arp(address, timestamp),
                IpAddress::Ipv6(address) => self.request_ndp(address, timestamp),
            }
        }
        if self.pending_packets.is_full() {
            warn!("Pending packets buffer is full, dropping packet");
            return false;
        }
        let Ok(dst_buffer) = self.pending_packets.enqueue(packet.len(), next_hop) else {
            warn!("Failed to enqueue packet in pending packets buffer");
            return false;
        };
        dst_buffer.copy_from_slice(packet);
        false
    }

    fn poll_complete(&mut self) -> bool {
        if let Err(err) = self.inner.recycle_tx_buffers() {
            if !matches!(err, DevError::Again) {
                warn!("recycle_tx_buffers failed: {:?}", err);
            }
        }
        self.inner.poll_complete()
    }

    fn register_waker(&self, waker: &Waker) {
        self.rx_wakers.register(waker);
        if let Some(irq) = self.inner.irq_num() {
            if self.inner.irq_handler_registered() {
                axtask::future::register_driver_irq_waker(irq, waker);
            } else {
                register_irq_waker(irq, waker);
            }
        }
    }
}
