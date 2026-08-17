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

use alloc::{borrow::ToOwned, boxed::Box};
use core::{
    net::Ipv4Addr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use axdriver::{AxDeviceContainer, prelude::*};
use axsync::Mutex;
pub use family::IpFamily;
use lazyinit::LazyInit;
use smoltcp::wire::{EthernetAddress, Ipv4Address, Ipv4Cidr, Ipv6Address, Ipv6Cidr};
pub use socket::*;

use crate::{
    consts::{GATEWAY, IP, IP_PREFIX},
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

/// The statically configured IPv4 state exposed to Linux compatibility APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticIpv4Config {
    pub address: Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Ipv4Addr,
    pub mac: [u8; 6],
}

/// Returns the configured `eth0` state once a physical NIC was registered.
pub fn static_ipv4_config() -> Option<StaticIpv4Config> {
    Some(StaticIpv4Config {
        address: IP.parse().ok()?,
        prefix_len: IP_PREFIX,
        gateway: GATEWAY.parse().ok()?,
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

    let eth0_ip = if let Some(dev) = net_devs.take_one() {
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
        let eth0_ip = Ipv4Cidr::new(IP.parse().expect("Invalid IPv4 address"), IP_PREFIX);

        let eth0_dev = router.add_device(Box::new(EthernetDevice::new(
            "eth0".to_owned(),
            dev,
            eth0_ip,
        )));

        router.add_rule(Rule::new(
            Ipv4Cidr::new(Ipv4Address::UNSPECIFIED, 0).into(),
            Some(GATEWAY.parse().expect("Invalid gateway address")),
            eth0_dev,
            eth0_ip.address().into(),
        ));

        info!("eth0:");
        info!("  mac:  {}", eth0_address);
        info!("  ip:   {}", eth0_ip);

        Some(eth0_ip)
    } else {
        warn!("  No network device found!");
        None
    };
    let has_physical_nic = eth0_ip.is_some();

    for dev in &router.devices {
        info!("Device: {}", dev.name());
    }

    let mut service = Service::new(router);
    service.iface.update_ip_addrs(|ip_addrs| {
        ip_addrs.push(lo_ip.into()).unwrap();
        ip_addrs.push(lo_ip6.into()).unwrap();
        if let Some(eth0_ip) = eth0_ip {
            ip_addrs.push(eth0_ip.into()).unwrap();
        }
    });
    SERVICE.init_once(Mutex::new(service));

    SOCKET_SET.init_once(SocketSetWrapper::new());
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
        warn!(
            "network poll budget exhausted; yielding with deferred work still pending"
        );
    }
}
