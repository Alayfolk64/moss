use alloc::vec::Vec;
use core::task::Waker;

use smoltcp::{
    storage::PacketBuffer,
    time::{Duration, Instant},
    wire::{IpAddress, IpCidr, Ipv6Address, Ipv6Cidr},
};

mod ethernet;
mod loopback;
#[cfg(feature = "vsock")]
mod vsock;

pub use ethernet::*;
pub use loopback::*;
#[cfg(feature = "vsock")]
pub use vsock::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NeighborState {
    Incomplete,
    Reachable,
    Stale,
    Permanent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NeighborEntry {
    pub address: IpAddress,
    pub hardware_address: Option<[u8; 6]>,
    pub state: NeighborState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ipv6AddressLease {
    pub address: Ipv6Cidr,
    pub valid_lifetime: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ipv6RouterLease {
    pub address: Ipv6Address,
    pub lifetime: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ipv6DnsServerLease {
    pub address: Ipv6Address,
    pub lifetime: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Ipv6AutoconfigEvent {
    /// `None` means this RA did not contain a usable autonomous prefix.
    /// A zero lifetime explicitly withdraws the contained address.
    pub address: Option<Ipv6AddressLease>,
    /// Every valid RA refreshes or withdraws its advertising router.
    pub router: Ipv6RouterLease,
    /// RDNSS entries advertised by this RA, including zero-lifetime removals.
    pub dns_servers: Vec<Ipv6DnsServerLease>,
}

pub trait Device: Send + Sync {
    fn name(&self) -> &str;

    fn is_loopback(&self) -> bool {
        false
    }

    fn notify_rx_ready(&self) {}

    fn update_ip_addrs(&mut self, _addrs: &[IpCidr]) {}

    fn neighbor_entries(&self, _timestamp: Instant, _entries: &mut Vec<NeighborEntry>) {}

    fn remove_neighbor(&mut self, _address: IpAddress) -> bool {
        false
    }

    fn set_neighbor(&mut self, _address: IpAddress, _hardware_address: [u8; 6]) -> bool {
        false
    }

    fn take_ipv6_autoconfig_event(&mut self) -> Option<Ipv6AutoconfigEvent> {
        None
    }

    fn recv(&mut self, buffer: &mut PacketBuffer<()>, timestamp: Instant) -> bool;
    /// Sends a packet to the next hop.
    ///
    /// Returns `true` if this operation resulted in the readiness of receive
    /// operation. This is true for loopback devices and can be used to speed
    /// up packet processing.
    fn send(&mut self, next_hop: IpAddress, packet: &[u8], timestamp: Instant) -> bool;

    /// Finishes one deferred device poll cycle.
    fn poll_complete(&mut self) -> bool {
        false
    }

    fn register_waker(&self, waker: &Waker);
}
