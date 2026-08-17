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
    device::NeighborEntry,
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
    slaac_enabled: bool,
    slaac_address: Option<Ipv6Address>,
    ipv6_router_discovery: bool,
    timeout: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

#[derive(Clone, Debug)]
pub(crate) struct NetworkSnapshot {
    pub addresses: Vec<IpCidr>,
    pub ipv4_gateway: Option<Ipv4Address>,
    pub ipv6_gateway: Option<Ipv6Address>,
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
            slaac_enabled,
            slaac_address: None,
            ipv6_router_discovery,
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
        NetworkSnapshot {
            addresses: self.eth0_addrs.clone(),
            ipv4_gateway: self.ipv4_gateway,
            ipv6_gateway: self.ipv6_gateway,
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
            IpAddress::Ipv4(gateway) => self.ipv4_gateway = Some(gateway),
            IpAddress::Ipv6(gateway) => self.ipv6_gateway = Some(gateway),
        }
        self.rebuild_network_state();
        Ok(())
    }

    pub(crate) fn remove_default_gateway(&mut self, ipv6: bool) -> AxResult {
        let removed = if ipv6 {
            self.ipv6_gateway.take().is_some()
        } else {
            self.ipv4_gateway.take().is_some()
        };
        if !removed {
            return Err(AxError::NotFound);
        }
        self.rebuild_network_state();
        Ok(())
    }

    fn apply_ipv6_autoconfig(&mut self) {
        let Some(dev) = self.eth0_dev else {
            return;
        };
        let Some(event) = self.router.devices[dev].take_ipv6_autoconfig_event() else {
            return;
        };
        let mut changed = false;
        if self.slaac_enabled
            && let Some(address) = event.address
            && self.slaac_address != Some(address.address())
        {
            if let Some(old) = self.slaac_address {
                self.eth0_addrs
                    .retain(|configured| configured.address() != IpAddress::Ipv6(old));
            }
            self.eth0_addrs.push(address.into());
            self.slaac_address = Some(address.address());
            changed = true;
        }
        if self.ipv6_router_discovery && event.router.is_some() && self.ipv6_gateway != event.router
        {
            self.ipv6_gateway = event.router;
            changed = true;
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
                self.eth0_addrs.push(address.into());
                self.dhcp_address = Some(address.address());
                self.ipv4_gateway = config
                    .router
                    .filter(|gateway| address.contains_addr(gateway));
                info!(
                    "DHCPv4 configured eth0: address={}, gateway={:?}, dns={:?}",
                    address, self.ipv4_gateway, config.dns_servers
                );
                self.rebuild_network_state();
            }
            dhcpv4::Event::Deconfigured => {
                if let Some(old) = self.dhcp_address.take() {
                    self.eth0_addrs
                        .retain(|configured| configured.address() != IpAddress::Ipv4(old));
                    self.ipv4_gateway = None;
                    info!("DHCPv4 deconfigured eth0");
                    self.rebuild_network_state();
                }
            }
        }
    }

    pub fn poll(&mut self, sockets: &mut SocketSet) -> bool {
        let timestamp = now();

        self.router.poll(timestamp);
        self.apply_ipv6_autoconfig();
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
