use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use smoltcp::wire::{IpAddress, IpEndpoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpFamily {
    Ipv4,
    Ipv6,
}

impl IpFamily {
    pub fn unspecified_addr(self) -> IpAddress {
        match self {
            Self::Ipv4 => IpAddress::Ipv4(Ipv4Addr::UNSPECIFIED.into()),
            Self::Ipv6 => IpAddress::Ipv6(Ipv6Addr::UNSPECIFIED.into()),
        }
    }

    pub fn loopback_addr(self) -> IpAddress {
        match self {
            Self::Ipv4 => IpAddress::Ipv4(Ipv4Addr::LOCALHOST.into()),
            Self::Ipv6 => IpAddress::Ipv6(Ipv6Addr::LOCALHOST.into()),
        }
    }

    pub fn format_socket_addr(self, addr: IpAddress, port: u16) -> SocketAddr {
        match (self, addr) {
            (Self::Ipv6, IpAddress::Ipv4(v4)) => {
                SocketAddrV6::new(v4_mapped_addr(v4.into()), port, 0, 0).into()
            }
            (_, addr) => SocketAddr::new(addr.into(), port),
        }
    }

    pub fn endpoint_from_socket_addr(
        self,
        addr: SocketAddr,
        ipv6_only: bool,
    ) -> Option<IpEndpoint> {
        match (self, addr) {
            (Self::Ipv4, SocketAddr::V4(v4)) => Some(v4.into()),
            (Self::Ipv6, SocketAddr::V6(v6)) => {
                if let Some(v4) = v6.ip().to_ipv4_mapped() {
                    (!ipv6_only).then(|| SocketAddrV4::new(v4, v6.port()).into())
                } else {
                    Some(v6.into())
                }
            }
            _ => None,
        }
    }
}

fn v4_mapped_addr(addr: Ipv4Addr) -> Ipv6Addr {
    addr.to_ipv6_mapped()
}
