//! `NETLINK_ROUTE` support for ordinary address, route, and neighbor operations.

use alloc::{collections::VecDeque, vec, vec::Vec};
use core::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::atomic::{AtomicU32, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axio::prelude::*;
use axpoll::{IoEvents, Pollable};
use smoltcp::wire::{IpAddress, IpCidr, Ipv4Cidr, Ipv6Cidr};
use spin::Mutex;

use crate::{
    RecvFlags, RecvOptions, SERVICE, SendOptions, Shutdown, SocketAddrEx, SocketOps,
    device::NeighborState,
    eth0_mac,
    general::GeneralOptions,
    local::inet_diagnostics,
    options::{Configurable, GetSocketOption, SetSocketOption},
    unix::{UnixSocketAddr, unix_diagnostics},
};

const AF_UNSPEC: u8 = 0;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_MULTI: u16 = 2;
const NLM_F_ACK: u16 = 4;
const NLM_F_DUMP: u16 = 0x300;

const RTM_NEWLINK: u16 = 16;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;
const RTM_NEWNEIGH: u16 = 28;
const RTM_DELNEIGH: u16 = 29;
const RTM_GETNEIGH: u16 = 30;

const NETLINK_SOCK_DIAG: u32 = 4;
const SOCK_DIAG_BY_FAMILY: u16 = 20;
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;
const UNIX_DIAG_NAME: u16 = 0;

const IFLA_ADDRESS: u16 = 1;
const IFLA_BROADCAST: u16 = 2;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_OPERSTATE: u16 = 16;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;
const IFA_BROADCAST: u16 = 4;

const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PREFSRC: u16 = 7;

const NDA_DST: u16 = 1;
const NDA_LLADDR: u16 = 2;

#[derive(Clone, Copy, Debug, Default)]
pub struct NetlinkAddr {
    pub pid: u32,
    pub groups: u32,
}

pub struct NetlinkSocket {
    protocol: u32,
    local_pid: AtomicU32,
    peer_pid: AtomicU32,
    responses: Mutex<VecDeque<Vec<u8>>>,
    general: GeneralOptions,
}

impl NetlinkSocket {
    pub fn new(pid: u32, protocol: u32) -> Self {
        Self {
            protocol,
            local_pid: AtomicU32::new(pid),
            peer_pid: AtomicU32::new(0),
            responses: Mutex::new(VecDeque::new()),
            general: GeneralOptions::new(),
        }
    }

    fn enqueue_request(&self, request: &[u8]) -> AxResult {
        if request.len() < 16 {
            return Err(AxError::InvalidInput);
        }
        let declared_len = read_u32(request, 0) as usize;
        if !(16..=request.len()).contains(&declared_len) {
            return Err(AxError::InvalidInput);
        }
        if self.protocol == NETLINK_SOCK_DIAG {
            return self.enqueue_diag_request(&request[..declared_len]);
        }
        let kind = read_u16(request, 4);
        let flags = read_u16(request, 6);
        let seq = read_u32(request, 8);
        let pid = self.local_pid.load(Ordering::Acquire);
        let family = request.get(16).copied().unwrap_or(AF_UNSPEC);
        let mut response = Vec::new();
        let is_dump = flags & NLM_F_DUMP == NLM_F_DUMP;
        match kind {
            RTM_GETLINK if is_dump => append_links(&mut response, seq, pid),
            RTM_GETLINK => {
                if request.len() < 24 {
                    return Err(AxError::InvalidInput);
                }
                append_link_by_index(&mut response, seq, pid, read_i32(request, 20))?;
            }
            RTM_GETADDR => append_addresses(&mut response, seq, pid, family),
            RTM_GETROUTE => append_routes(&mut response, seq, pid, family),
            RTM_GETNEIGH => append_neighbors(&mut response, seq, pid, family),
            RTM_NEWADDR | RTM_DELADDR | RTM_NEWROUTE | RTM_DELROUTE | RTM_NEWNEIGH
            | RTM_DELNEIGH => {
                let result = apply_network_change(kind, &request[..declared_len]);
                let error = result
                    .err()
                    .map_or(0, |error| -LinuxError::from(error).code());
                // Netlink reports failures unconditionally, but a successful
                // NLMSG_ERROR acknowledgement is emitted only when the sender
                // asked for NLM_F_ACK. BusyBox neighbor flush deliberately
                // omits ACK and treats any immediately readable NLMSG_ERROR,
                // including error=0, as a failed flush.
                if error != 0 || flags & NLM_F_ACK != 0 {
                    append_error(&mut response, seq, pid, &request, error);
                }
            }
            _ => append_error(&mut response, seq, pid, request, -95),
        }
        let mut responses = self.responses.lock();
        if is_dump
            && matches!(
                kind,
                RTM_GETLINK | RTM_GETADDR | RTM_GETROUTE | RTM_GETNEIGH
            )
        {
            if !response.is_empty() {
                responses.push_back(response);
            }
            let mut done = Vec::new();
            append_done(&mut done, seq, pid);
            responses.push_back(done);
        } else if !response.is_empty() {
            responses.push_back(response);
        }
        Ok(())
    }

    fn enqueue_diag_request(&self, request: &[u8]) -> AxResult {
        if request.len() < 24 || read_u16(request, 4) != SOCK_DIAG_BY_FAMILY {
            return Err(AxError::InvalidInput);
        }
        let seq = read_u32(request, 8);
        let pid = self.local_pid.load(Ordering::Acquire);
        let family = request[16];
        let protocol = request[17];
        let states = read_u32(request, 20);
        let mut response = Vec::new();

        match family {
            AF_INET | AF_INET6 if protocol == IPPROTO_TCP || protocol == IPPROTO_UDP => {
                append_inet_diagnostics(&mut response, seq, pid, family, protocol, states);
            }
            1 => append_unix_diagnostics(&mut response, seq, pid, states),
            _ => {
                append_error(&mut response, seq, pid, request, -95);
                self.responses.lock().push_back(response);
                return Ok(());
            }
        }

        let mut responses = self.responses.lock();
        if !response.is_empty() {
            responses.push_back(response);
        }
        let mut done = Vec::new();
        append_done(&mut done, seq, pid);
        responses.push_back(done);
        Ok(())
    }
}

impl Configurable for NetlinkSocket {
    fn get_option_inner(&self, option: &mut GetSocketOption) -> AxResult<bool> {
        self.general.get_option_inner(option)
    }

    fn set_option_inner(&self, option: SetSocketOption) -> AxResult<bool> {
        self.general.set_option_inner(option)
    }
}

impl SocketOps for NetlinkSocket {
    fn bind(&self, local_addr: SocketAddrEx) -> AxResult {
        let SocketAddrEx::Netlink(addr) = local_addr else {
            return Err(AxError::from(LinuxError::EAFNOSUPPORT));
        };
        if addr.groups != 0 {
            return Err(AxError::OperationNotSupported);
        }
        if addr.pid != 0 {
            self.local_pid.store(addr.pid, Ordering::Release);
        }
        Ok(())
    }

    fn connect(&self, remote_addr: SocketAddrEx) -> AxResult {
        let SocketAddrEx::Netlink(addr) = remote_addr else {
            return Err(AxError::from(LinuxError::EAFNOSUPPORT));
        };
        if addr.groups != 0 {
            return Err(AxError::OperationNotSupported);
        }
        self.peer_pid.store(addr.pid, Ordering::Release);
        Ok(())
    }

    fn send(&self, mut src: impl Read + IoBuf, _options: SendOptions) -> AxResult<usize> {
        let len = src.remaining();
        if len > 64 * 1024 {
            return Err(AxError::from(LinuxError::EMSGSIZE));
        }
        let mut request = vec![0; len];
        src.read_exact(&mut request)?;

        // A netlink datagram may contain multiple aligned messages. BusyBox
        // batches the RTM_DELNEIGH requests generated by `ip neigh flush`, so
        // silently handling only the first header both leaves stale entries
        // behind and makes the command reject the incomplete acknowledgement.
        let mut offset = 0;
        while offset < request.len() {
            // iproute2 sends some dump requests from a fixed-size request
            // structure: nlmsg_len covers the initialized message while the
            // rest of the datagram is a zeroed attribute scratch buffer.
            // Linux stops parsing at that padding. Keep rejecting malformed
            // non-zero tails so a damaged batched request is not hidden.
            if request[offset..].iter().all(|byte| *byte == 0) {
                break;
            }
            if request.len() - offset < 16 {
                return Err(AxError::InvalidInput);
            }
            let declared_len = read_u32(&request, offset) as usize;
            if declared_len < 16 || declared_len > request.len() - offset {
                return Err(AxError::InvalidInput);
            }
            self.enqueue_request(&request[offset..offset + declared_len])?;
            offset = offset
                .checked_add(align4(declared_len))
                .ok_or(AxError::InvalidInput)?;
        }
        Ok(len)
    }

    fn recv(
        &self,
        mut dst: impl Write + IoBufMut,
        mut options: RecvOptions<'_>,
    ) -> AxResult<usize> {
        let mut responses = self.responses.lock();
        let Some(response) = responses.front() else {
            return Err(AxError::WouldBlock);
        };
        if let Some(from) = options.from.as_mut() {
            **from = SocketAddrEx::Netlink(NetlinkAddr { pid: 0, groups: 0 });
        }
        let full_len = response.len();
        let write_len = full_len.min(dst.remaining_mut());
        let written = dst.write(&response[..write_len])?;
        if !options.flags.contains(RecvFlags::PEEK) {
            responses.pop_front();
        }
        Ok(if options.flags.contains(RecvFlags::TRUNCATE) {
            full_len
        } else {
            written
        })
    }

    fn local_addr(&self) -> AxResult<SocketAddrEx> {
        Ok(SocketAddrEx::Netlink(NetlinkAddr {
            pid: self.local_pid.load(Ordering::Acquire),
            groups: 0,
        }))
    }

    fn peer_addr(&self) -> AxResult<SocketAddrEx> {
        Ok(SocketAddrEx::Netlink(NetlinkAddr {
            pid: self.peer_pid.load(Ordering::Acquire),
            groups: 0,
        }))
    }

    fn shutdown(&self, _how: Shutdown) -> AxResult {
        self.responses.lock().clear();
        Ok(())
    }
}

impl Pollable for NetlinkSocket {
    fn poll(&self) -> IoEvents {
        let mut events = IoEvents::OUT;
        if !self.responses.lock().is_empty() {
            events |= IoEvents::IN;
        }
        events
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

fn align4(len: usize) -> usize {
    (len + 3) & !3
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_i32(data: &[u8], offset: usize) -> i32 {
    i32::from_ne_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn push_u16(data: &mut Vec<u8>, value: u16) {
    data.extend_from_slice(&value.to_ne_bytes());
}

fn push_u32(data: &mut Vec<u8>, value: u32) {
    data.extend_from_slice(&value.to_ne_bytes());
}

fn push_i32(data: &mut Vec<u8>, value: i32) {
    data.extend_from_slice(&value.to_ne_bytes());
}

fn append_attr(message: &mut Vec<u8>, kind: u16, value: &[u8]) {
    let len = 4 + value.len();
    push_u16(message, len as u16);
    push_u16(message, kind);
    message.extend_from_slice(value);
    message.resize(align4(message.len()), 0);
}

fn append_message(
    output: &mut Vec<u8>,
    kind: u16,
    flags: u16,
    seq: u32,
    pid: u32,
    payload: impl FnOnce(&mut Vec<u8>),
) {
    let start = output.len();
    output.resize(start + 16, 0);
    payload(output);
    let len = output.len() - start;
    output[start..start + 4].copy_from_slice(&(len as u32).to_ne_bytes());
    output[start + 4..start + 6].copy_from_slice(&kind.to_ne_bytes());
    output[start + 6..start + 8].copy_from_slice(&flags.to_ne_bytes());
    output[start + 8..start + 12].copy_from_slice(&seq.to_ne_bytes());
    output[start + 12..start + 16].copy_from_slice(&pid.to_ne_bytes());
    output.resize(start + align4(len), 0);
}

fn append_inet_diagnostics(
    output: &mut Vec<u8>,
    seq: u32,
    pid: u32,
    family: u8,
    protocol: u8,
    states: u32,
) {
    for diagnostic in inet_diagnostics() {
        let diagnostic_family = match diagnostic.family {
            crate::IpFamily::Ipv4 => AF_INET,
            crate::IpFamily::Ipv6 => AF_INET6,
        };
        if diagnostic_family != family
            || diagnostic.protocol != protocol
            || states & (1_u32 << diagnostic.state) == 0
        {
            continue;
        }

        let address = diagnostic
            .endpoint
            .addr
            .unwrap_or_else(|| diagnostic.family.unspecified_addr());
        append_message(
            output,
            SOCK_DIAG_BY_FAMILY,
            NLM_F_MULTI,
            seq,
            pid,
            |message| {
                message.extend_from_slice(&[diagnostic_family, diagnostic.state, 0, 0]);
                message.extend_from_slice(&diagnostic.endpoint.port.to_be_bytes());
                message.extend_from_slice(&0_u16.to_be_bytes());
                match address {
                    IpAddress::Ipv4(address) => {
                        message.extend_from_slice(&address.octets());
                        message.extend_from_slice(&[0; 12]);
                    }
                    IpAddress::Ipv6(address) => message.extend_from_slice(&address.octets()),
                }
                message.extend_from_slice(&[0; 16]);
                push_u32(message, 0);
                push_u32(message, u32::MAX);
                push_u32(message, u32::MAX);
                push_u32(message, 0);
                push_u32(message, diagnostic.rx_queue);
                push_u32(message, 0);
                push_u32(message, 0);
                push_u32(message, 0);
            },
        );
    }
}

fn append_unix_diagnostics(output: &mut Vec<u8>, seq: u32, pid: u32, states: u32) {
    for diagnostic in unix_diagnostics() {
        if states & (1_u32 << diagnostic.state) == 0 {
            continue;
        }
        append_message(
            output,
            SOCK_DIAG_BY_FAMILY,
            NLM_F_MULTI,
            seq,
            pid,
            |message| {
                message.extend_from_slice(&[1, diagnostic.socket_type, diagnostic.state, 0]);
                push_u32(message, diagnostic.inode);
                push_u32(message, u32::MAX);
                push_u32(message, u32::MAX);
                match diagnostic.address {
                    UnixSocketAddr::Unnamed => {}
                    UnixSocketAddr::Abstract(name) => {
                        let mut value = Vec::with_capacity(name.len() + 1);
                        value.push(0);
                        value.extend_from_slice(&name);
                        append_attr(message, UNIX_DIAG_NAME, &value);
                    }
                    UnixSocketAddr::Path(path) => {
                        append_attr(message, UNIX_DIAG_NAME, path.as_bytes());
                    }
                }
            },
        );
    }
}

fn append_link(
    output: &mut Vec<u8>,
    seq: u32,
    pid: u32,
    index: i32,
    device_type: u16,
    flags: u32,
    name: &[u8],
    mtu: u32,
    address: &[u8],
    message_flags: u16,
) {
    append_message(output, RTM_NEWLINK, message_flags, seq, pid, |message| {
        message.push(AF_UNSPEC);
        message.push(0);
        push_u16(message, device_type);
        push_i32(message, index);
        push_u32(message, flags);
        push_u32(message, 0);
        append_attr(message, IFLA_IFNAME, name);
        append_attr(message, IFLA_MTU, &mtu.to_ne_bytes());
        append_attr(message, IFLA_OPERSTATE, &[6]);
        append_attr(message, IFLA_ADDRESS, address);
        let broadcast = if device_type == 1 { [0xff; 6] } else { [0; 6] };
        append_attr(message, IFLA_BROADCAST, &broadcast);
    });
}

fn append_links(output: &mut Vec<u8>, seq: u32, pid: u32) {
    append_link(
        output,
        seq,
        pid,
        1,
        772,
        0x49,
        b"lo\0",
        65_536,
        &[0; 6],
        NLM_F_MULTI,
    );
    if let Some(mac) = eth0_mac() {
        append_link(
            output,
            seq,
            pid,
            2,
            1,
            0x1043,
            b"eth0\0",
            1500,
            &mac,
            NLM_F_MULTI,
        );
    }
}

fn append_link_by_index(output: &mut Vec<u8>, seq: u32, pid: u32, index: i32) -> AxResult {
    match index {
        1 => append_link(output, seq, pid, 1, 772, 0x49, b"lo\0", 65_536, &[0; 6], 0),
        2 => {
            let mac = eth0_mac().ok_or(AxError::NoSuchDevice)?;
            append_link(output, seq, pid, 2, 1, 0x1043, b"eth0\0", 1500, &mac, 0);
        }
        _ => return Err(AxError::NoSuchDevice),
    }
    Ok(())
}

fn append_address(
    output: &mut Vec<u8>,
    seq: u32,
    pid: u32,
    family: u8,
    prefix: u8,
    scope: u8,
    index: u32,
    address: &[u8],
    label: &[u8],
    broadcast: Option<&[u8]>,
) {
    append_message(output, RTM_NEWADDR, NLM_F_MULTI, seq, pid, |message| {
        message.extend_from_slice(&[family, prefix, 0x80, scope]);
        push_u32(message, index);
        append_attr(message, IFA_ADDRESS, address);
        append_attr(message, IFA_LOCAL, address);
        append_attr(message, IFA_LABEL, label);
        if let Some(broadcast) = broadcast {
            append_attr(message, IFA_BROADCAST, broadcast);
        }
    });
}

fn append_addresses(output: &mut Vec<u8>, seq: u32, pid: u32, family: u8) {
    if family == AF_UNSPEC || family == AF_INET {
        append_address(
            output,
            seq,
            pid,
            AF_INET,
            8,
            254,
            1,
            &[127, 0, 0, 1],
            b"lo\0",
            None,
        );
    }
    if family == AF_UNSPEC || family == AF_INET6 {
        let mut loopback = [0; 16];
        loopback[15] = 1;
        append_address(
            output, seq, pid, AF_INET6, 128, 254, 1, &loopback, b"lo\0", None,
        );
    }

    let Some(service) = SERVICE.get() else {
        return;
    };
    let snapshot = service.lock().network_snapshot();
    for address in snapshot.addresses {
        match address {
            IpCidr::Ipv4(address) if family == AF_UNSPEC || family == AF_INET => {
                let broadcast = address.broadcast().map(|address| address.octets());
                append_address(
                    output,
                    seq,
                    pid,
                    AF_INET,
                    address.prefix_len(),
                    0,
                    2,
                    &address.address().octets(),
                    b"eth0\0",
                    broadcast.as_ref().map(<[u8; 4]>::as_slice),
                );
            }
            IpCidr::Ipv6(address) if family == AF_UNSPEC || family == AF_INET6 => {
                let ip = address.address();
                append_address(
                    output,
                    seq,
                    pid,
                    AF_INET6,
                    address.prefix_len(),
                    if ip.is_unicast_link_local() { 253 } else { 0 },
                    2,
                    &ip.octets(),
                    b"eth0\0",
                    None,
                );
            }
            _ => {}
        }
    }
}

fn ipv6_network(address: Ipv6Cidr) -> [u8; 16] {
    let mut network = address.address().octets();
    let whole_bytes = usize::from(address.prefix_len() / 8);
    let remaining_bits = address.prefix_len() % 8;
    if whole_bytes < network.len() {
        if remaining_bits != 0 {
            network[whole_bytes] &= u8::MAX << (8 - remaining_bits);
        }
        let clear_from = whole_bytes + usize::from(remaining_bits != 0);
        network[clear_from..].fill(0);
    }
    network
}

#[allow(clippy::too_many_arguments)]
fn append_route(
    output: &mut Vec<u8>,
    seq: u32,
    pid: u32,
    family: u8,
    prefix: u8,
    protocol: u8,
    scope: u8,
    destination: Option<&[u8]>,
    gateway: Option<&[u8]>,
    preferred_source: Option<&[u8]>,
) {
    append_message(output, RTM_NEWROUTE, NLM_F_MULTI, seq, pid, |message| {
        message.extend_from_slice(&[family, prefix, 0, 0, 254, protocol, scope, 1]);
        push_u32(message, 0);
        if let Some(destination) = destination {
            append_attr(message, RTA_DST, destination);
        }
        append_attr(message, RTA_OIF, &2_u32.to_ne_bytes());
        if let Some(gateway) = gateway {
            append_attr(message, RTA_GATEWAY, gateway);
        }
        if let Some(source) = preferred_source {
            append_attr(message, RTA_PREFSRC, source);
        }
    });
}

fn append_routes(output: &mut Vec<u8>, seq: u32, pid: u32, family: u8) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let snapshot = service.lock().network_snapshot();
    for address in &snapshot.addresses {
        match address {
            IpCidr::Ipv4(address) if family == AF_UNSPEC || family == AF_INET => {
                let destination = address.network().address().octets();
                let source = address.address().octets();
                append_route(
                    output,
                    seq,
                    pid,
                    AF_INET,
                    address.prefix_len(),
                    2,
                    253,
                    Some(&destination),
                    None,
                    Some(&source),
                );
            }
            IpCidr::Ipv6(address) if family == AF_UNSPEC || family == AF_INET6 => {
                let destination = ipv6_network(*address);
                let source = address.address().octets();
                append_route(
                    output,
                    seq,
                    pid,
                    AF_INET6,
                    address.prefix_len(),
                    2,
                    253,
                    Some(&destination),
                    None,
                    Some(&source),
                );
            }
            _ => {}
        }
    }

    for route in &snapshot.static_routes {
        match route.destination {
            IpCidr::Ipv4(destination) if family == AF_UNSPEC || family == AF_INET => {
                let destination = destination.network().address().octets();
                let gateway = match route.gateway {
                    Some(IpAddress::Ipv4(gateway)) => Some(gateway.octets()),
                    _ => None,
                };
                append_route(
                    output,
                    seq,
                    pid,
                    AF_INET,
                    route.destination.prefix_len(),
                    4,
                    if gateway.is_some() { 0 } else { 253 },
                    Some(&destination),
                    gateway.as_ref().map(<[u8; 4]>::as_slice),
                    None,
                );
            }
            IpCidr::Ipv6(destination) if family == AF_UNSPEC || family == AF_INET6 => {
                let destination = ipv6_network(destination);
                let gateway = match route.gateway {
                    Some(IpAddress::Ipv6(gateway)) => Some(gateway.octets()),
                    _ => None,
                };
                append_route(
                    output,
                    seq,
                    pid,
                    AF_INET6,
                    route.destination.prefix_len(),
                    4,
                    if gateway.is_some() { 0 } else { 253 },
                    Some(&destination),
                    gateway.as_ref().map(<[u8; 16]>::as_slice),
                    None,
                );
            }
            _ => {}
        }
    }

    if (family == AF_UNSPEC || family == AF_INET)
        && let Some(gateway) = snapshot.ipv4_gateway
    {
        append_route(
            output,
            seq,
            pid,
            AF_INET,
            0,
            3,
            0,
            None,
            Some(&gateway.octets()),
            None,
        );
    }
    if (family == AF_UNSPEC || family == AF_INET6)
        && let Some(gateway) = snapshot.ipv6_gateway
    {
        append_route(
            output,
            seq,
            pid,
            AF_INET6,
            0,
            3,
            0,
            None,
            Some(&gateway.octets()),
            None,
        );
    }
}

fn append_neighbors(output: &mut Vec<u8>, seq: u32, pid: u32, family: u8) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let entries = service.lock().neighbor_entries();
    for entry in entries {
        let (entry_family, address) = match entry.address {
            IpAddress::Ipv4(address) => (AF_INET, address.octets().to_vec()),
            IpAddress::Ipv6(address) => (AF_INET6, address.octets().to_vec()),
        };
        if family != AF_UNSPEC && family != entry_family {
            continue;
        }
        let state = match entry.state {
            NeighborState::Incomplete => 0x01,
            NeighborState::Reachable => 0x02,
            NeighborState::Stale => 0x04,
            NeighborState::Permanent => 0x80,
        };
        append_message(output, RTM_NEWNEIGH, NLM_F_MULTI, seq, pid, |message| {
            message.extend_from_slice(&[entry_family, 0]);
            push_u16(message, 0);
            push_i32(message, 2);
            push_u16(message, state);
            message.extend_from_slice(&[0, 1]);
            append_attr(message, NDA_DST, &address);
            if let Some(hardware_address) = entry.hardware_address {
                append_attr(message, NDA_LLADDR, &hardware_address);
            }
        });
    }
}

fn find_attr(message: &[u8], mut offset: usize, kind: u16) -> Option<&[u8]> {
    while offset.checked_add(4)? <= message.len() {
        let length = usize::from(read_u16(message, offset));
        if length < 4 || offset.checked_add(length)? > message.len() {
            return None;
        }
        if read_u16(message, offset + 2) == kind {
            return Some(&message[offset + 4..offset + length]);
        }
        offset = offset.checked_add(align4(length))?;
    }
    None
}

fn apply_network_change(kind: u16, request: &[u8]) -> AxResult {
    match kind {
        RTM_NEWADDR | RTM_DELADDR => apply_address_change(kind, request),
        RTM_NEWROUTE | RTM_DELROUTE => apply_route_change(kind, request),
        RTM_NEWNEIGH | RTM_DELNEIGH => apply_neighbor_change(kind, request),
        _ => Err(AxError::OperationNotSupported),
    }
}

fn apply_neighbor_change(kind: u16, request: &[u8]) -> AxResult {
    if request.len() < 28 || read_i32(request, 20) != 2 {
        return Err(AxError::NoSuchDevice);
    }
    let bytes = find_attr(request, 28, NDA_DST).ok_or(AxError::InvalidInput)?;
    let address = match request[16] {
        AF_INET if bytes.len() == 4 => {
            IpAddress::Ipv4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).unwrap()))
        }
        AF_INET6 if bytes.len() == 16 => {
            IpAddress::Ipv6(Ipv6Addr::from(<[u8; 16]>::try_from(bytes).unwrap()))
        }
        _ => return Err(AxError::InvalidInput),
    };
    let service = SERVICE.get().ok_or(AxError::NoSuchDevice)?;
    if kind == RTM_DELNEIGH {
        return service.lock().remove_neighbor(address);
    }
    let hardware_address = find_attr(request, 28, NDA_LLADDR).ok_or(AxError::InvalidInput)?;
    if hardware_address.len() != 6 {
        return Err(AxError::InvalidInput);
    }
    service
        .lock()
        .set_neighbor(address, <[u8; 6]>::try_from(hardware_address).unwrap())
}

fn apply_address_change(kind: u16, request: &[u8]) -> AxResult {
    if request.len() < 24 || read_u32(request, 20) != 2 {
        return Err(AxError::NoSuchDevice);
    }
    let family = request[16];
    let prefix = request[17];
    let bytes = find_attr(request, 24, IFA_LOCAL)
        .or_else(|| find_attr(request, 24, IFA_ADDRESS))
        .ok_or(AxError::InvalidInput)?;
    let address = match family {
        AF_INET if bytes.len() == 4 && prefix <= 32 => {
            let address = Ipv4Addr::from(<[u8; 4]>::try_from(bytes).unwrap());
            if kind == RTM_DELADDR {
                IpAddress::Ipv4(address)
            } else {
                let cidr = Ipv4Cidr::new(address, prefix);
                return SERVICE
                    .get()
                    .ok_or(AxError::NoSuchDevice)?
                    .lock()
                    .set_address(cidr.into());
            }
        }
        AF_INET6 if bytes.len() == 16 && prefix <= 128 => {
            let address = Ipv6Addr::from(<[u8; 16]>::try_from(bytes).unwrap());
            if kind == RTM_DELADDR {
                IpAddress::Ipv6(address)
            } else {
                let cidr = Ipv6Cidr::new(address, prefix);
                return SERVICE
                    .get()
                    .ok_or(AxError::NoSuchDevice)?
                    .lock()
                    .set_address(cidr.into());
            }
        }
        _ => return Err(AxError::InvalidInput),
    };
    SERVICE
        .get()
        .ok_or(AxError::NoSuchDevice)?
        .lock()
        .remove_address(address)
}

fn apply_route_change(kind: u16, request: &[u8]) -> AxResult {
    if request.len() < 28 || request[18] != 0 {
        return Err(AxError::OperationNotSupported);
    }
    if let Some(output_interface) = find_attr(request, 28, RTA_OIF) {
        if output_interface.len() != 4 || read_u32(output_interface, 0) != 2 {
            return Err(AxError::NoSuchDevice);
        }
    }
    let family = request[16];
    let prefix = request[17];
    let destination = find_attr(request, 28, RTA_DST);
    let destination = match family {
        AF_INET if prefix <= 32 => {
            let bytes = destination.unwrap_or(&[0; 4]);
            if bytes.len() != 4 {
                return Err(AxError::InvalidInput);
            }
            IpCidr::Ipv4(Ipv4Cidr::new(
                Ipv4Addr::from(<[u8; 4]>::try_from(bytes).unwrap()),
                prefix,
            ))
        }
        AF_INET6 if prefix <= 128 => {
            let bytes = destination.unwrap_or(&[0; 16]);
            if bytes.len() != 16 {
                return Err(AxError::InvalidInput);
            }
            IpCidr::Ipv6(Ipv6Cidr::new(
                Ipv6Addr::from(<[u8; 16]>::try_from(bytes).unwrap()),
                prefix,
            ))
        }
        _ => return Err(AxError::InvalidInput),
    };
    let service = SERVICE.get().ok_or(AxError::NoSuchDevice)?;
    if kind == RTM_DELROUTE {
        if prefix != 0 {
            return service.lock().remove_route(destination);
        }
        return match family {
            AF_INET => service.lock().remove_default_gateway(false),
            AF_INET6 => service.lock().remove_default_gateway(true),
            _ => Err(AxError::InvalidInput),
        };
    }

    let gateway = match (family, find_attr(request, 28, RTA_GATEWAY)) {
        (AF_INET, Some(gateway)) if gateway.len() == 4 => Some(IpAddress::Ipv4(Ipv4Addr::from(
            <[u8; 4]>::try_from(gateway).unwrap(),
        ))),
        (AF_INET6, Some(gateway)) if gateway.len() == 16 => Some(IpAddress::Ipv6(Ipv6Addr::from(
            <[u8; 16]>::try_from(gateway).unwrap(),
        ))),
        (_, None) => None,
        _ => return Err(AxError::InvalidInput),
    };
    if prefix == 0 {
        service
            .lock()
            .set_default_gateway(gateway.ok_or(AxError::InvalidInput)?)
    } else {
        service.lock().set_route(destination, gateway)
    }
}

fn append_done(output: &mut Vec<u8>, seq: u32, pid: u32) {
    append_message(output, NLMSG_DONE, NLM_F_MULTI, seq, pid, |message| {
        push_i32(message, 0);
    });
}

fn append_error(output: &mut Vec<u8>, seq: u32, pid: u32, request: &[u8], error: i32) {
    append_message(output, NLMSG_ERROR, 0, seq, pid, |message| {
        push_i32(message, error);
        message.extend_from_slice(&request[..16]);
    });
}
