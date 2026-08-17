macro_rules! env_or_default {
    ($key:literal) => {
        match option_env!($key) {
            Some(val) => val,
            None => "",
        }
    };
}

pub const IPV4_ADDRESS: &str = env_or_default!("AX_IP");
pub const IPV4_PREFIX: &str = env_or_default!("AX_IP_PREFIX");
pub const IPV4_GATEWAY: &str = env_or_default!("AX_GW");
pub const IPV6_ADDRESS: &str = env_or_default!("AX_IP6");
pub const IPV6_PREFIX: &str = env_or_default!("AX_IP6_PREFIX");
pub const IPV6_GATEWAY: &str = env_or_default!("AX_GW6");

pub const STANDARD_MTU: usize = 1500;
pub const LOOPBACK_MTU: usize = 65_535;
pub const LOOPBACK_ADVERTISED_MSS: usize = 65_535 - 20 - 20;
pub const ROUTER_PACKET_BUFFER_SIZE: usize = 256;

pub const TCP_RX_BUF_LEN: usize = 512 * 1024;
pub const TCP_TX_BUF_LEN: usize = 512 * 1024;
pub const TCP_SEND_POLL_BYTES: usize = 128 * 1024;
pub const UDP_RX_BUF_LEN: usize = 1024 * 1024;
pub const UDP_TX_BUF_LEN: usize = 1024 * 1024;
pub const UDP_PACKET_BUFFER_SIZE: usize = 1024;
pub const UDP_LOOPBACK_YIELD_BYTES: usize = 128 * 1024;
pub const MAX_SOCKET_BUF_LEN: usize = 8 * 1024 * 1024;
pub const LISTEN_QUEUE_SIZE: usize = 512;

pub const SOCKET_BUFFER_SIZE: usize = 1024;
pub const ETHERNET_MAX_PENDING_PACKETS: usize = 32;
