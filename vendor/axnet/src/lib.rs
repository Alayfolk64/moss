//! [ArceOS](https://github.com/rcore-os/arceos) network module.
//!
//! It provides unified networking primitives for TCP/UDP communication
//! using various underlying network stacks. Currently, only [smoltcp] is
//! supported.
//!
//! # Organization
//!
//! - [`TcpSocket`]: A TCP socket that provides POSIX-like APIs.
//! - [`UdpSocket`]: A UDP socket that provides POSIX-like APIs.
//! - [`dns_query`]: Function for DNS query.
//!
//! [smoltcp]: https://github.com/smoltcp-rs/smoltcp

#![no_std]
#![feature(maybe_uninit_slice)]

#[macro_use]
extern crate log;
extern crate alloc;

mod consts;
mod device;
mod family;
mod general;
mod listen_table;
mod local;
pub mod netlink;
pub mod options;
pub mod raw;
mod router;
mod service;
mod socket;
pub(crate) mod state;
pub mod tcp;
pub mod udp;
pub mod unix;
#[cfg(feature = "vsock")]
pub mod vsock;
mod wrapper;

use alloc::{borrow::ToOwned, boxed::Box, vec::Vec};
use core::{
    net::Ipv4Addr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use axdriver::{AxDeviceContainer, prelude::*};
use axsync::Mutex;
pub use family::IpFamily;
use lazyinit::LazyInit;
use smoltcp::wire::{
    EthernetAddress, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr, Ipv6Address, Ipv6Cidr,
};
pub use socket::*;

use crate::{
    consts::{IPV4_ADDRESS, IPV4_GATEWAY, IPV4_PREFIX, IPV6_ADDRESS, IPV6_GATEWAY, IPV6_PREFIX},
    device::{EthernetDevice, LoopbackDevice},
    listen_table::ListenTable,
    router::{Router, Rule},
    service::Service,
    wrapper::SocketSetWrapper,
};

static LISTEN_TABLE: LazyInit<ListenTable> = LazyInit::new();
static SOCKET_SET: LazyInit<SocketSetWrapper> = LazyInit::new();

static SERVICE: LazyInit<Mutex<Service>> = LazyInit::new();
static ETH0_MAC: AtomicU64 = AtomicU64::new(0);
static POLL_BUDGET_WARNING_REPORTED: AtomicBool = AtomicBool::new(false);

/// Keep a noisy or faulty device from monopolizing the network service lock.
/// The interrupt path and the one-second link poll will schedule subsequent
/// work, so exhausting this budget is a yield point rather than packet loss.
const MAX_SERVICE_POLL_ROUNDS: usize = 64;

pub(crate) fn eth0_mac() -> Option<[u8; 6]> {
    let encoded = ETH0_MAC.load(Ordering::Acquire);
    (encoded != 0).then(|| {
        let bytes = encoded.to_be_bytes();
        [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]
    })
}

fn parse_prefix(value: &str, default: u8, maximum: u8, name: &str) -> u8 {
    let prefix = if value.is_empty() {
        default
    } else {
        value
            .parse::<u8>()
            .unwrap_or_else(|_| panic!("Invalid {name}: {value}"))
    };
    assert!(prefix <= maximum, "Invalid {name}: {prefix} > {maximum}");
    prefix
}

fn ipv6_link_local(mac: EthernetAddress) -> Ipv6Address {
    let mut interface = [0; 8];
    interface[0] = mac.0[0] ^ 0x02;
    interface[1] = mac.0[1];
    interface[2] = mac.0[2];
    interface[3] = 0xff;
    interface[4] = 0xfe;
    interface[5] = mac.0[3];
    interface[6] = mac.0[4];
    interface[7] = mac.0[5];
    Ipv6Address::from([
        0xfe,
        0x80,
        0,
        0,
        0,
        0,
        0,
        0,
        interface[0],
        interface[1],
        interface[2],
        interface[3],
        interface[4],
        interface[5],
        interface[6],
        interface[7],
    ])
}

fn configured_addresses(mac: EthernetAddress) -> Vec<IpCidr> {
    let mut addresses: Vec<IpCidr> = Vec::new();
    if !IPV4_ADDRESS.is_empty() {
        let address: Ipv4Address = IPV4_ADDRESS
            .parse()
            .unwrap_or_else(|_| panic!("Invalid IPv4 address: {IPV4_ADDRESS}"));
        addresses
            .push(Ipv4Cidr::new(address, parse_prefix(IPV4_PREFIX, 24, 32, "IPv4 prefix")).into());
    }

    addresses.push(Ipv6Cidr::new(ipv6_link_local(mac), 64).into());
    if !IPV6_ADDRESS.is_empty() {
        let address: Ipv6Address = IPV6_ADDRESS
            .parse()
            .unwrap_or_else(|_| panic!("Invalid IPv6 address: {IPV6_ADDRESS}"));
        let cidr = Ipv6Cidr::new(address, parse_prefix(IPV6_PREFIX, 64, 128, "IPv6 prefix"));
        if !addresses
            .iter()
            .any(|configured| configured.address() == IpAddress::Ipv6(address))
        {
            addresses.push(cidr.into());
        }
    }
    addresses
}

fn configured_ipv4_gateway(addresses: &[IpCidr]) -> Option<Ipv4Address> {
    if IPV4_GATEWAY.is_empty() {
        return None;
    }
    let gateway: Ipv4Address = IPV4_GATEWAY
        .parse()
        .unwrap_or_else(|_| panic!("Invalid IPv4 gateway: {IPV4_GATEWAY}"));
    assert!(
        addresses
            .iter()
            .any(|address| matches!(address, IpCidr::Ipv4(cidr) if cidr.contains_addr(&gateway))),
        "IPv4 gateway {gateway} is outside the configured IPv4 subnet"
    );
    Some(gateway)
}

fn configured_ipv6_gateway(addresses: &[IpCidr]) -> Option<Ipv6Address> {
    if IPV6_GATEWAY.is_empty() {
        return None;
    }
    let gateway: Ipv6Address = IPV6_GATEWAY
        .parse()
        .unwrap_or_else(|_| panic!("Invalid IPv6 gateway: {IPV6_GATEWAY}"));
    assert!(
        addresses
            .iter()
            .any(|address| matches!(address, IpCidr::Ipv6(cidr) if cidr.contains_addr(&gateway))),
        "IPv6 gateway {gateway} is outside the configured IPv6 subnet"
    );
    Some(gateway)
}

/// The current primary IPv4 state exposed to Linux compatibility APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Config {
    pub address: Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Option<Ipv4Addr>,
    pub mac: [u8; 6],
}

pub fn eth0_exists() -> bool {
    eth0_mac().is_some()
}

pub fn eth0_mac_address() -> Option<[u8; 6]> {
    eth0_mac()
}

/// Returns the current primary IPv4 configuration, if `eth0` has one.
pub fn ipv4_config() -> Option<Ipv4Config> {
    let snapshot = SERVICE.get()?.lock().network_snapshot();
    let address = snapshot
        .addresses
        .iter()
        .find_map(|address| match address {
            IpCidr::Ipv4(address) => Some(*address),
            _ => None,
        })?;
    Some(Ipv4Config {
        address: address.address(),
        prefix_len: address.prefix_len(),
        gateway: snapshot.ipv4_gateway,
        mac: eth0_mac()?,
    })
}

/// Initializes the network subsystem by NIC devices.
pub fn init_network(mut net_devs: AxDeviceContainer<AxNetDevice>) {
    info!("Initialize network subsystem...");

    let mut router = Router::new();
    let lo_dev = router.add_device(Box::new(LoopbackDevice::new()));

    let lo_ip = Ipv4Cidr::new(Ipv4Address::new(127, 0, 0, 1), 8);
    router.add_rule(Rule::new(
        lo_ip.into(),
        None,
        lo_dev,
        lo_ip.address().into(),
    ));
    let lo_ip6 = Ipv6Cidr::new(Ipv6Address::LOCALHOST, 128);
    router.add_rule(Rule::new(
        lo_ip6.into(),
        None,
        lo_dev,
        lo_ip6.address().into(),
    ));

    let mut eth0_addrs = Vec::new();
    let mut ipv4_gateway = None;
    let mut ipv6_gateway = None;
    let eth0_dev = if let Some(dev) = net_devs.take_one() {
        info!("  use NIC 0: {:?}", dev.device_name());

        let eth0_address = EthernetAddress(dev.mac_address().0);
        ETH0_MAC.store(
            u64::from_be_bytes([
                0,
                0,
                eth0_address.0[0],
                eth0_address.0[1],
                eth0_address.0[2],
                eth0_address.0[3],
                eth0_address.0[4],
                eth0_address.0[5],
            ]),
            Ordering::Release,
        );
        eth0_addrs = configured_addresses(eth0_address);
        ipv4_gateway = configured_ipv4_gateway(&eth0_addrs);
        ipv6_gateway = configured_ipv6_gateway(&eth0_addrs);

        let eth0_dev = router.add_device(Box::new(EthernetDevice::new(
            "eth0".to_owned(),
            dev,
            &eth0_addrs,
        )));

        info!("eth0:");
        info!("  mac:  {}", eth0_address);
        for address in &eth0_addrs {
            info!("  ip:   {}", address);
        }

        Some(eth0_dev)
    } else {
        warn!("  No network device found!");
        None
    };
    let has_physical_nic = eth0_dev.is_some();

    for dev in &router.devices {
        info!("Device: {}", dev.name());
    }

    SOCKET_SET.init_once(SocketSetWrapper::new());
    let dhcp_handle = if has_physical_nic
        && !eth0_addrs
            .iter()
            .any(|address| matches!(address, IpCidr::Ipv4(_)))
    {
        let mut socket = smoltcp::socket::dhcpv4::Socket::new();
        socket.set_client_hardware_address(EthernetAddress(eth0_mac().unwrap()));
        Some(SOCKET_SET.add(socket))
    } else {
        None
    };
    let service = Service::new(
        router,
        eth0_dev,
        eth0_addrs,
        ipv4_gateway,
        ipv6_gateway,
        dhcp_handle,
        IPV6_ADDRESS.is_empty(),
        IPV6_GATEWAY.is_empty(),
    );
    SERVICE.init_once(Mutex::new(service));

    LISTEN_TABLE.init_once(ListenTable::new());
    local::init();
    if has_physical_nic {
        axtask::spawn_with_name(network_link_poll_loop, "net-link-poll".to_owned());
    }
}

/// PHYs may not provide a usable link-change interrupt through the MAC. Keep
/// Linux phylib's one-second state-machine cadence even while all sockets are
/// idle, so cable and negotiated-mode changes reach the driver promptly.
fn network_link_poll_loop() {
    loop {
        axtask::future::block_on(axtask::future::sleep(Duration::from_secs(1)));
        poll_interfaces();
    }
}

/// Init vsock subsystem by vsock devices.
#[cfg(feature = "vsock")]
pub fn init_vsock(mut vsock_devs: AxDeviceContainer<AxVsockDevice>) {
    use crate::device::register_vsock_device;
    info!("Initialize vsock subsystem...");
    if let Some(dev) = vsock_devs.take_one() {
        info!("  use vsock 0: {:?}", dev.device_name());
        if let Err(e) = register_vsock_device(dev) {
            warn!("Failed to initialize vsock device: {:?}", e);
        }
    } else {
        warn!("  No vsock device found!");
    }
}

pub fn poll_interfaces() {
    for _ in 0..MAX_SERVICE_POLL_ROUNDS {
        if !SERVICE.lock().poll(&mut SOCKET_SET.inner.lock()) {
            return;
        }
    }
    if !POLL_BUDGET_WARNING_REPORTED.swap(true, Ordering::AcqRel) {
        warn!("network poll budget exhausted; yielding with deferred work still pending");
    }
}
