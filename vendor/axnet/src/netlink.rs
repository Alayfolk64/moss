//! Minimal read-only `NETLINK_ROUTE` support for ordinary iproute2 queries.

use alloc::{collections::VecDeque, vec, vec::Vec};
use core::{
    net::Ipv4Addr,
    sync::atomic::{AtomicU32, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult, LinuxError};
use axio::prelude::*;
use axpoll::{IoEvents, Pollable};
use spin::Mutex;

use crate::{
    RecvFlags, RecvOptions, SendOptions, Shutdown, SocketAddrEx, SocketOps,
    consts::{GATEWAY, IP, IP_PREFIX},
    eth0_mac,
    general::GeneralOptions,
    options::{Configurable, GetSocketOption, SetSocketOption},
};

const AF_UNSPEC: u8 = 0;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_MULTI: u16 = 2;
const NLM_F_DUMP: u16 = 0x300;

const RTM_NEWLINK: u16 = 16;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;

const IFLA_ADDRESS: u16 = 1;
const IFLA_BROADCAST: u16 = 2;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_OPERSTATE: u16 = 16;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;
const IFA_BROADCAST: u16 = 4;

const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;

#[derive(Clone, Copy, Debug, Default)]
pub struct NetlinkAddr {
    pub pid: u32,
    pub groups: u32,
}

pub struct NetlinkSocket {
    local_pid: AtomicU32,
    peer_pid: AtomicU32,
    responses: Mutex<VecDeque<Vec<u8>>>,
    general: GeneralOptions,
}

impl NetlinkSocket {
    pub fn new(pid: u32) -> Self {
        Self {
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
            _ => append_error(&mut response, seq, pid, request, -95),
        }
        let mut responses = self.responses.lock();
        if is_dump && matches!(kind, RTM_GETLINK | RTM_GETADDR | RTM_GETROUTE) {
            if !response.is_empty() {
                responses.push_back(response);
            }
            let mut done = Vec::new();
            append_done(&mut done, seq, pid);
            responses.push_back(done);
        } else {
            responses.push_back(response);
        }
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
        self.enqueue_request(&request)?;
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
        if eth0_mac().is_some() {
            let ip: Ipv4Addr = IP.parse().expect("invalid configured IPv4 address");
            let mask = if IP_PREFIX == 0 {
                0
            } else {
                u32::MAX << (32 - IP_PREFIX)
            };
            let broadcast = Ipv4Addr::from(u32::from(ip) | !mask).octets();
            append_address(
                output,
                seq,
                pid,
                AF_INET,
                IP_PREFIX,
                0,
                2,
                &ip.octets(),
                b"eth0\0",
                Some(&broadcast),
            );
        }
    }
    if family == AF_UNSPEC || family == AF_INET6 {
        let mut loopback = [0; 16];
        loopback[15] = 1;
        append_address(
            output, seq, pid, AF_INET6, 128, 254, 1, &loopback, b"lo\0", None,
        );
    }
}

fn append_routes(output: &mut Vec<u8>, seq: u32, pid: u32, family: u8) {
    if (family == AF_UNSPEC || family == AF_INET) && eth0_mac().is_some() {
        append_message(output, RTM_NEWROUTE, NLM_F_MULTI, seq, pid, |message| {
            message.extend_from_slice(&[AF_INET, 0, 0, 0, 254, 3, 0, 1]);
            push_u32(message, 0);
            append_attr(message, RTA_OIF, &2_u32.to_ne_bytes());
            let gateway: Ipv4Addr = GATEWAY.parse().expect("invalid configured gateway");
            append_attr(message, RTA_GATEWAY, &gateway.octets());
        });
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
