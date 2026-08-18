use alloc::{boxed::Box, vec::Vec};
use core::{
    pin::Pin,
    task::{Context, Waker},
    time::Duration,
};

use axerrno::{AxError, AxResult, LinuxError};
use axhal::time::{NANOS_PER_MICROS, TimeValue, wall_time, wall_time_nanos};
use axtask::future::sleep_until;
use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    socket::dhcpv4,
    time::Instant,
    wire::{
        HardwareAddress, IpAddress, IpCidr, IpListenEndpoint, Ipv4Address, Ipv4Cidr, Ipv6Address,
        Ipv6Cidr,
    },
};

use crate::{
    SOCKET_SET,
    device::{Ipv6DnsServerLease, NeighborEntry},
    router::{Router, Rule},
};

/// Keep blocking socket operations making progress even on platforms where a
/// network device interrupt is unavailable or routed incorrectly.
const MAX_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn now() -> Instant {
    Instant::from_micros_const((wall_time_nanos() / NANOS_PER_MICROS) as i64)
}

pub struct Service {
    pub iface: Interface,
    router: Router,
    eth0_dev: Option<usize>,
    eth0_addrs: Vec<IpCidr>,
    ipv4_gateway: Option<Ipv4Address>,
    ipv6_gateway: Option<Ipv6Address>,
    dhcp_handle: Option<SocketHandle>,
    dhcp_address: Option<Ipv4Address>,
    dhcp_gateway: Option<Ipv4Address>,
    dhcp_dns_servers: Vec<Ipv4Address>,
    slaac_enabled: bool,
    slaac_address: Option<Ipv6Address>,
    slaac_valid_until: Option<Instant>,
    ipv6_router_discovery: bool,
    ipv6_gateway_valid_until: Option<Instant>,
    ipv6_dns_servers: Vec<(Ipv6Address, Option<Instant>)>,
    static_routes: Vec<ConfiguredRoute>,
    timeout: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConfiguredRoute {
    pub destination: IpCidr,
    pub gateway: Option<IpAddress>,
}

#[derive(Clone, Debug)]
pub(crate) struct NetworkSnapshot {
    pub addresses: Vec<IpCidr>,
    pub ipv4_gateway: Option<Ipv4Address>,
    pub ipv6_gateway: Option<Ipv6Address>,
    pub dns_servers: Vec<IpAddress>,
    pub static_routes: Vec<ConfiguredRoute>,
}

impl Service {
    pub fn new(
        mut router: Router,
        eth0_dev: Option<usize>,
        eth0_addrs: Vec<IpCidr>,
        ipv4_gateway: Option<Ipv4Address>,
        ipv6_gateway: Option<Ipv6Address>,
        dhcp_handle: Option<SocketHandle>,
        slaac_enabled: bool,
        ipv6_router_discovery: bool,
    ) -> Self {
        let config = smoltcp::iface::Config::new(HardwareAddress::Ip);
        let iface = Interface::new(config, &mut router, now());

        let mut service = Self {
            iface,
            router,
            eth0_dev,
            eth0_addrs,
            ipv4_gateway,
            ipv6_gateway,
            dhcp_handle,
            dhcp_address: None,
            dhcp_gateway: None,
            dhcp_dns_servers: Vec::new(),
            slaac_enabled,
            slaac_address: None,
            slaac_valid_until: None,
            ipv6_router_discovery,
            ipv6_gateway_valid_until: None,
            ipv6_dns_servers: Vec::new(),
            static_routes: Vec::new(),
            timeout: None,
        };
        service.rebuild_network_state();
        service
    }

    fn rebuild_network_state(&mut self) {
        let Some(dev) = self.eth0_dev else {
            return;
        };

        self.router.table.remove_device_rules(dev);
        for address in &self.eth0_addrs {
            self.router
                .add_rule(Rule::new(*address, None, dev, address.address()));
        }

        for route in &self.static_routes {
            let source = route.gateway.and_then(|gateway| {
                self.eth0_addrs
                    .iter()
                    .find(|address| address.contains_addr(&gateway))
                    .map(IpCidr::address)
            });
            let source = source.or_else(|| {
                self.eth0_addrs.iter().find_map(|address| {
                    matches!(
                        (route.destination, *address),
                        (IpCidr::Ipv4(_), IpCidr::Ipv4(_))
                            | (IpCidr::Ipv6(_), IpCidr::Ipv6(_))
                    )
                    .then(|| address.address())
                })
            });
            if let Some(source) = source {
                self.router
                    .add_rule(Rule::new(route.destination, route.gateway, dev, source));
            }
        }

        if let Some(gateway) = self.ipv4_gateway {
            if let Some(source) = self.eth0_addrs.iter().find_map(|address| match address {
                IpCidr::Ipv4(address) if address.contains_addr(&gateway) => Some(address.address()),
                _ => None,
            }) {
                self.router.add_rule(Rule::new(
                    Ipv4Cidr::new(Ipv4Address::UNSPECIFIED, 0).into(),
                    Some(gateway.into()),
                    dev,
                    source.into(),
                ));
            }
        }

        if let (Some(gateway), Some(source)) = (
            self.ipv6_gateway,
            self.eth0_addrs
                .iter()
                .filter_map(|address| match address {
                    IpCidr::Ipv6(address) => Some(address.address()),
                    _ => None,
                })
                .find(|address| !address.is_unicast_link_local())
                .or_else(|| {
                    self.eth0_addrs.iter().find_map(|address| match address {
                        IpCidr::Ipv6(address) => Some(address.address()),
                        _ => None,
                    })
                }),
        ) {
            self.router.add_rule(Rule::new(
                Ipv6Cidr::new(Ipv6Address::UNSPECIFIED, 0).into(),
                Some(gateway.into()),
                dev,
                source.into(),
            ));
        }

        self.iface.update_ip_addrs(|addresses| {
            addresses.clear();
            addresses
                .push(Ipv4Cidr::new(Ipv4Address::LOCALHOST, 8).into())
                .unwrap();
            addresses
                .push(Ipv6Cidr::new(Ipv6Address::LOCALHOST, 128).into())
                .unwrap();
            for address in &self.eth0_addrs {
                addresses
                    .push(*address)
                    .expect("too many configured IP addresses");
            }
        });
        self.router.devices[dev].update_ip_addrs(&self.eth0_addrs);
    }

    pub(crate) fn network_snapshot(&self) -> NetworkSnapshot {
        let mut dns_servers = Vec::new();
        dns_servers.extend(self.dhcp_dns_servers.iter().copied().map(IpAddress::Ipv4));
        for (address, _) in &self.ipv6_dns_servers {
            let address = IpAddress::Ipv6(*address);
            if !dns_servers.contains(&address) {
                dns_servers.push(address);
            }
        }
        NetworkSnapshot {
            addresses: self.eth0_addrs.clone(),
            ipv4_gateway: self.ipv4_gateway,
            ipv6_gateway: self.ipv6_gateway,
            dns_servers,
            static_routes: self.static_routes.clone(),
        }
    }

    pub(crate) fn neighbor_entries(&self) -> Vec<NeighborEntry> {
        let mut entries = Vec::new();
        if let Some(dev) = self.eth0_dev {
            self.router.devices[dev].neighbor_entries(now(), &mut entries);
        }
        entries
    }

    pub(crate) fn remove_neighbor(&mut self, address: IpAddress) -> AxResult {
        let Some(dev) = self.eth0_dev else {
            return Err(AxError::NoSuchDevice);
        };
        if self.router.devices[dev].remove_neighbor(address) {
            Ok(())
        } else {
            Err(AxError::NotFound)
        }
    }

    pub(crate) fn set_neighbor(
        &mut self,
        address: IpAddress,
        hardware_address: [u8; 6],
    ) -> AxResult {
        let Some(dev) = self.eth0_dev else {
            return Err(AxError::NoSuchDevice);
        };
        if self.router.devices[dev].set_neighbor(address, hardware_address) {
            Ok(())
        } else {
            Err(AxError::InvalidInput)
        }
    }

    pub(crate) fn set_address(&mut self, address: IpCidr) -> AxResult {
        if address.address().is_unspecified()
            || address.address().is_multicast()
            || address.address().is_broadcast()
        {
            return Err(AxError::InvalidInput);
        }
        if let Some(current) = self
            .eth0_addrs
            .iter_mut()
            .find(|current| current.address() == address.address())
        {
            *current = address;
        } else {
            if self.eth0_addrs.len() >= 6 {
                return Err(AxError::NoMemory);
            }
            self.eth0_addrs.push(address);
        }
        self.rebuild_network_state();
        Ok(())
    }

    pub(crate) fn remove_address(&mut self, address: IpAddress) -> AxResult {
        let old_len = self.eth0_addrs.len();
        self.eth0_addrs
            .retain(|configured| configured.address() != address);
        if self.eth0_addrs.len() == old_len {
            return Err(AxError::NotFound);
        }
        if self.ipv4_gateway.is_some_and(|gateway| {
            !self.eth0_addrs.iter().any(
                |address| matches!(address, IpCidr::Ipv4(cidr) if cidr.contains_addr(&gateway)),
            )
        }) {
            self.ipv4_gateway = None;
        }
        if self.ipv6_gateway.is_some_and(|gateway| {
            !self.eth0_addrs.iter().any(
                |address| matches!(address, IpCidr::Ipv6(cidr) if cidr.contains_addr(&gateway)),
            )
        }) {
            self.ipv6_gateway = None;
        }
        self.rebuild_network_state();
        Ok(())
    }

    pub(crate) fn set_default_gateway(&mut self, gateway: IpAddress) -> AxResult {
        let on_link = self
            .eth0_addrs
            .iter()
            .any(|address| address.contains_addr(&gateway));
        if !on_link || gateway.is_unspecified() || gateway.is_multicast() {
            return Err(AxError::from(LinuxError::ENETUNREACH));
        }
        match gateway {
            IpAddress::Ipv4(gateway) => {
                self.ipv4_gateway = Some(gateway);
                self.dhcp_gateway = None;
            }
            IpAddress::Ipv6(gateway) => {
                self.ipv6_gateway = Some(gateway);
                self.ipv6_gateway_valid_until = None;
            }
        }
        self.rebuild_network_state();
        Ok(())
    }

    pub(crate) fn remove_default_gateway(&mut self, ipv6: bool) -> AxResult {
        let removed = if ipv6 {
            self.ipv6_gateway_valid_until = None;
            self.ipv6_gateway.take().is_some()
        } else {
            self.dhcp_gateway = None;
            self.ipv4_gateway.take().is_some()
        };
        if !removed {
            return Err(AxError::NotFound);
        }
        self.rebuild_network_state();
        Ok(())
    }

    pub(crate) fn set_route(
        &mut self,
        destination: IpCidr,
        gateway: Option<IpAddress>,
    ) -> AxResult {
        if destination.prefix_len() == 0 {
            return Err(AxError::InvalidInput);
        }
        let same_family = gateway.is_none_or(|gateway| {
            matches!(
                (destination, gateway),
                (IpCidr::Ipv4(_), IpAddress::Ipv4(_))
                    | (IpCidr::Ipv6(_), IpAddress::Ipv6(_))
            )
        });
        let gateway_on_link = gateway.is_none_or(|gateway| {
            !gateway.is_unspecified()
                && !gateway.is_multicast()
                && self
                    .eth0_addrs
                    .iter()
                    .any(|address| address.contains_addr(&gateway))
        });
        let has_source = self.eth0_addrs.iter().any(|address| {
            matches!(
                (destination, *address),
                (IpCidr::Ipv4(_), IpCidr::Ipv4(_)) | (IpCidr::Ipv6(_), IpCidr::Ipv6(_))
            )
        });
        if !same_family || !gateway_on_link || !has_source {
            return Err(AxError::from(LinuxError::ENETUNREACH));
        }

        let route = ConfiguredRoute {
            destination,
            gateway,
        };
        if let Some(current) = self
            .static_routes
            .iter_mut()
            .find(|current| current.destination == destination)
        {
            *current = route;
        } else {
            self.static_routes.push(route);
        }
        self.rebuild_network_state();
        Ok(())
    }

    pub(crate) fn remove_route(&mut self, destination: IpCidr) -> AxResult {
        let old_len = self.static_routes.len();
        self.static_routes
            .retain(|route| route.destination != destination);
        if self.static_routes.len() == old_len {
            return Err(AxError::NotFound);
        }
        self.rebuild_network_state();
        Ok(())
    }

    fn ipv6_lease_deadline(
        timestamp: Instant,
        lifetime: smoltcp::time::Duration,
    ) -> Option<Instant> {
        // RFC 4862 and RFC 8106 reserve all-one-bits as an infinite lifetime.
        (lifetime.secs() != u64::from(u32::MAX)).then(|| timestamp + lifetime)
    }

    fn update_ipv6_dns_server(&mut self, lease: Ipv6DnsServerLease, timestamp: Instant) {
        if lease.lifetime == smoltcp::time::Duration::ZERO {
            self.ipv6_dns_servers
                .retain(|(address, _)| *address != lease.address);
            return;
        }
        let valid_until = Self::ipv6_lease_deadline(timestamp, lease.lifetime);
        if let Some((_, current_valid_until)) = self
            .ipv6_dns_servers
            .iter_mut()
            .find(|(address, _)| *address == lease.address)
        {
            *current_valid_until = valid_until;
        } else {
            self.ipv6_dns_servers.push((lease.address, valid_until));
        }
    }

    fn expire_ipv6_autoconfig(&mut self, timestamp: Instant) {
        let mut changed = false;
        if self
            .slaac_valid_until
            .is_some_and(|valid_until| valid_until <= timestamp)
        {
            if let Some(address) = self.slaac_address.take() {
                self.eth0_addrs
                    .retain(|configured| configured.address() != IpAddress::Ipv6(address));
                changed = true;
            }
            self.slaac_valid_until = None;
        }
        if self
            .ipv6_gateway_valid_until
            .is_some_and(|valid_until| valid_until <= timestamp)
        {
            self.ipv6_gateway = None;
            self.ipv6_gateway_valid_until = None;
            changed = true;
        }
        self.ipv6_dns_servers.retain(|(_, valid_until)| {
            valid_until.is_none_or(|valid_until| valid_until > timestamp)
        });
        if changed {
            self.rebuild_network_state();
        }
    }

    fn apply_ipv6_autoconfig(&mut self, timestamp: Instant) {
        let Some(dev) = self.eth0_dev else {
            return;
        };
        let Some(event) = self.router.devices[dev].take_ipv6_autoconfig_event() else {
            return;
        };
        let mut changed = false;
        if self.slaac_enabled
            && let Some(lease) = event.address
        {
            let address = lease.address.address();
            if lease.valid_lifetime == smoltcp::time::Duration::ZERO {
                if self.slaac_address == Some(address) {
                    self.eth0_addrs
                        .retain(|configured| configured.address() != IpAddress::Ipv6(address));
                    self.slaac_address = None;
                    self.slaac_valid_until = None;
                    changed = true;
                }
            } else {
                if self.slaac_address != Some(address) {
                    if let Some(old) = self.slaac_address {
                        self.eth0_addrs
                            .retain(|configured| configured.address() != IpAddress::Ipv6(old));
                    }
                    if self.eth0_addrs.len() < 6 {
                        self.eth0_addrs.push(lease.address.into());
                        self.slaac_address = Some(address);
                        changed = true;
                    } else {
                        warn!(
                            "Ignoring SLAAC address {}: address table is full",
                            lease.address
                        );
                    }
                }
                if self.slaac_address == Some(address) {
                    self.slaac_valid_until =
                        Self::ipv6_lease_deadline(timestamp, lease.valid_lifetime);
                }
            }
        }
        if self.ipv6_router_discovery {
            if event.router.lifetime == smoltcp::time::Duration::ZERO {
                if self.ipv6_gateway == Some(event.router.address) {
                    self.ipv6_gateway = None;
                    self.ipv6_gateway_valid_until = None;
                    changed = true;
                }
            } else {
                if self.ipv6_gateway != Some(event.router.address) {
                    self.ipv6_gateway = Some(event.router.address);
                    changed = true;
                }
                self.ipv6_gateway_valid_until = Some(timestamp + event.router.lifetime);
            }
        }
        for lease in event.dns_servers {
            self.update_ipv6_dns_server(lease, timestamp);
        }
        if changed {
            self.rebuild_network_state();
        }
    }

    fn apply_dhcp_event(&mut self, event: dhcpv4::Event<'_>) {
        match event {
            dhcpv4::Event::Configured(config) => {
                let address = config.address;
                if let Some(old) = self.dhcp_address {
                    self.eth0_addrs
                        .retain(|configured| configured.address() != IpAddress::Ipv4(old));
                }
                self.dhcp_address = None;
                if !self
                    .eth0_addrs
                    .iter()
                    .any(|configured| configured.address() == IpAddress::Ipv4(address.address()))
                {
                    if self.eth0_addrs.len() >= 6 {
                        self.dhcp_gateway = None;
                        self.dhcp_dns_servers.clear();
                        warn!(
                            "Ignoring DHCPv4 lease {}: address table is full",
                            address
                        );
                        self.rebuild_network_state();
                        return;
                    }
                    self.eth0_addrs.push(address.into());
                    self.dhcp_address = Some(address.address());
                }
                self.dhcp_gateway = config
                    .router
                    .filter(|gateway| address.contains_addr(gateway));
                self.ipv4_gateway = self.dhcp_gateway;
                self.dhcp_dns_servers.clear();
                self.dhcp_dns_servers.extend(
                    config
                        .dns_servers
                        .iter()
                        .copied()
                        .filter(|address| !address.is_unspecified() && !address.is_multicast()),
                );
                info!(
                    "DHCPv4 configured eth0: address={}, gateway={:?}, dns={:?}",
                    address, self.ipv4_gateway, self.dhcp_dns_servers
                );
                self.rebuild_network_state();
            }
            dhcpv4::Event::Deconfigured => {
                let mut changed = false;
                if let Some(old) = self.dhcp_address.take() {
                    self.eth0_addrs
                        .retain(|configured| configured.address() != IpAddress::Ipv4(old));
                    changed = true;
                }
                if self.ipv4_gateway == self.dhcp_gateway && self.dhcp_gateway.is_some() {
                    self.ipv4_gateway = None;
                    changed = true;
                }
                self.dhcp_gateway = None;
                if !self.dhcp_dns_servers.is_empty() {
                    self.dhcp_dns_servers.clear();
                }
                if changed {
                    self.rebuild_network_state();
                }
                info!("DHCPv4 deconfigured eth0");
            }
        }
    }

    pub fn poll(&mut self, sockets: &mut SocketSet) -> bool {
        let timestamp = now();

        self.router.poll(timestamp);
        self.expire_ipv6_autoconfig(timestamp);
        self.apply_ipv6_autoconfig(timestamp);
        self.iface.poll(timestamp, &mut self.router, sockets);
        let dhcp_event = self
            .dhcp_handle
            .and_then(|handle| sockets.get_mut::<dhcpv4::Socket>(handle).poll());
        if let Some(event) = dhcp_event {
            self.apply_dhcp_event(event);
        }
        self.router.dispatch(timestamp)
    }

    pub fn get_source_address(&self, dst_addr: &IpAddress) -> AxResult<IpAddress> {
        let Some(rule) = self.router.table.lookup(dst_addr) else {
            return Err(AxError::from(LinuxError::ENETUNREACH));
        };
        Ok(rule.src)
    }

    pub fn can_bind_ip(&self, addr: &IpAddress) -> bool {
        addr.is_unspecified()
            || addr.is_multicast()
            || addr.is_broadcast()
            || self.iface.has_ip_addr(*addr)
    }

    pub fn device_mask_for(&self, endpoint: &IpListenEndpoint) -> u32 {
        match endpoint.addr {
            Some(addr) => self
                .router
                .table
                .lookup(&addr)
                .map_or(0, |it| 1u32 << it.dev),
            None => u32::MAX,
        }
    }

    pub fn notify_route_device(&self, dst_addr: &IpAddress) {
        if let Some(rule) = self.router.table.lookup(dst_addr) {
            self.router.devices[rule.dev].notify_rx_ready();
        }
    }

    pub fn register_waker(&mut self, mask: u32, waker: &Waker) {
        let protocol_deadline = self
            .iface
            .poll_at(now(), &SOCKET_SET.inner.lock())
            .map(|t| TimeValue::from_micros(t.total_micros() as _));
        let fallback_deadline = wall_time() + MAX_POLL_INTERVAL;
        let next = protocol_deadline.map_or(fallback_deadline, |t| t.min(fallback_deadline));
        // Never invoke a socket waker synchronously while SERVICE is locked.
        // A deadline returned at or just before `now` is deferred by one
        // scheduler tick; the next poll still happens promptly but outside
        // this critical section.
        let minimum_deadline = wall_time() + Duration::from_millis(1);
        let next = next.max(minimum_deadline);

        // Drop the previous timeout future and arm this socket's waker. The
        // bounded fallback is needed for devices that cannot wake the stack
        // through an RX interrupt (notably virtio-net on LA QEMU).
        self.timeout = None;

        let mut fut = Box::pin(sleep_until(next));
        let mut cx = Context::from_waker(waker);

        if fut.as_mut().poll(&mut cx).is_pending() {
            self.timeout = Some(fut);
        }

        for (i, device) in self.router.devices.iter().enumerate() {
            if mask & (1 << i) != 0 {
                device.register_waker(waker);
            }
        }
    }
}
