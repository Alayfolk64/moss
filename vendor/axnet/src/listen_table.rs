use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec, vec::Vec};

use axerrno::{AxError, AxResult};
use axsync::Mutex;
use smoltcp::{
    iface::{SocketHandle, SocketSet},
    socket::tcp::{self, SocketBuffer, State},
    wire::{IpAddress, IpEndpoint, IpListenEndpoint},
};

use crate::{IpFamily, SOCKET_SET, consts::LISTEN_QUEUE_SIZE};

const PORT_NUM: usize = 65536;

struct ListenTableEntry {
    listen_endpoint: IpListenEndpoint,
    family: IpFamily,
    ipv6_only: bool,
    tcp_rx_buf_len: usize,
    tcp_tx_buf_len: usize,
    syn_queue: VecDeque<SocketHandle>,
}

impl ListenTableEntry {
    pub fn new(
        listen_endpoint: IpListenEndpoint,
        family: IpFamily,
        ipv6_only: bool,
        tcp_rx_buf_len: usize,
        tcp_tx_buf_len: usize,
    ) -> Self {
        Self {
            listen_endpoint,
            family,
            ipv6_only,
            tcp_rx_buf_len,
            tcp_tx_buf_len,
            syn_queue: VecDeque::with_capacity(LISTEN_QUEUE_SIZE),
        }
    }

    fn accepts_packet(&self, dst: IpEndpoint) -> bool {
        if self.listen_endpoint.port != dst.port {
            return false;
        }

        match (self.listen_endpoint.addr, dst.addr) {
            (Some(listen_addr), dst_addr) => listen_addr == dst_addr,
            (None, IpAddress::Ipv4(_)) => self.family == IpFamily::Ipv4 || !self.ipv6_only,
            (None, IpAddress::Ipv6(_)) => self.family == IpFamily::Ipv6,
        }
    }
}

impl Drop for ListenTableEntry {
    fn drop(&mut self) {
        for &handle in &self.syn_queue {
            SOCKET_SET.remove(handle);
        }
    }
}

pub struct ListenTable {
    tcp: Box<[Arc<Mutex<Vec<Box<ListenTableEntry>>>>]>,
}

impl ListenTable {
    pub fn new() -> Self {
        let tcp = unsafe {
            let mut buf = Box::new_uninit_slice(PORT_NUM);
            for i in 0..PORT_NUM {
                buf[i].write(Arc::default());
            }
            buf.assume_init()
        };
        Self { tcp }
    }

    pub fn can_listen(&self, port: u16) -> bool {
        self.tcp[port as usize].lock().is_empty()
    }

    pub fn listen(
        &self,
        listen_endpoint: IpListenEndpoint,
        family: IpFamily,
        ipv6_only: bool,
        tcp_rx_buf_len: usize,
        tcp_tx_buf_len: usize,
    ) -> AxResult {
        let port = listen_endpoint.port;
        assert_ne!(port, 0);
        let mut entries = self.tcp[port as usize].lock();
        if entries.iter().any(|entry| {
            listeners_overlap(
                entry.listen_endpoint,
                entry.family,
                entry.ipv6_only,
                listen_endpoint,
                family,
                ipv6_only,
            )
        }) {
            warn!("socket already listening on port {port}");
            return Err(AxError::AddrInUse);
        }
        entries.push(Box::new(ListenTableEntry::new(
            listen_endpoint,
            family,
            ipv6_only,
            tcp_rx_buf_len,
            tcp_tx_buf_len,
        )));
        Ok(())
    }

    pub fn unlisten(&self, listen_endpoint: IpListenEndpoint, family: IpFamily, ipv6_only: bool) {
        debug!("TCP socket unlisten on {}", listen_endpoint);
        self.tcp[listen_endpoint.port as usize]
            .lock()
            .retain(|entry| !entry.matches_listener(listen_endpoint, family, ipv6_only));
    }

    fn listen_entries(&self, port: u16) -> Arc<Mutex<Vec<Box<ListenTableEntry>>>> {
        self.tcp[port as usize].clone()
    }

    pub fn can_accept(
        &self,
        listen_endpoint: IpListenEndpoint,
        family: IpFamily,
        ipv6_only: bool,
    ) -> AxResult<bool> {
        let entries = self.listen_entries(listen_endpoint.port);
        let entries = entries.lock();
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.matches_listener(listen_endpoint, family, ipv6_only))
        else {
            warn!("accept before listen");
            return Err(AxError::InvalidInput);
        };
        Ok(entry.syn_queue.iter().any(|&handle| is_connected(handle)))
    }

    pub fn accept(
        &self,
        listen_endpoint: IpListenEndpoint,
        family: IpFamily,
        ipv6_only: bool,
    ) -> AxResult<SocketHandle> {
        let entries = self.listen_entries(listen_endpoint.port);
        let mut entries = entries.lock();
        let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.matches_listener(listen_endpoint, family, ipv6_only))
        else {
            warn!("accept before listen");
            return Err(AxError::InvalidInput);
        };

        let syn_queue: &mut VecDeque<SocketHandle> = &mut entry.syn_queue;
        let idx = syn_queue
            .iter()
            .enumerate()
            .find_map(|(idx, &handle)| is_connected(handle).then_some(idx))
            .ok_or(AxError::WouldBlock)?; // wait for connection
        if idx > 0 {
            warn!(
                "slow SYN queue enumeration: index = {}, len = {}!",
                idx,
                syn_queue.len()
            );
        }
        let handle = syn_queue.swap_remove_front(idx).unwrap();
        // If the connection is reset, return ConnectionReset error
        // Otherwise, return the handle and the address tuple
        if is_closed(handle) {
            warn!("accept failed: connection reset");
            Err(AxError::ConnectionReset)
        } else {
            Ok(handle)
        }
    }

    pub fn incoming_tcp_packet(
        &self,
        src: IpEndpoint,
        dst: IpEndpoint,
        sockets: &mut SocketSet<'_>,
    ) {
        let entries = self.listen_entries(dst.port);
        let mut entries = entries.lock();
        if let Some(entry) = entries.iter_mut().find(|entry| entry.accepts_packet(dst)) {
            // TODO(mivik): accept address check
            if entry.syn_queue.len() >= LISTEN_QUEUE_SIZE {
                // SYN queue is full, drop the packet
                warn!("SYN queue overflow!");
                return;
            }

            let mut socket = smoltcp::socket::tcp::Socket::new(
                SocketBuffer::new(vec![0; entry.tcp_rx_buf_len]),
                SocketBuffer::new(vec![0; entry.tcp_tx_buf_len]),
            );
            if let Err(err) = socket.listen(IpListenEndpoint {
                addr: None,
                port: dst.port,
            }) {
                warn!("Failed to listen on {}: {:?}", entry.listen_endpoint, err);
                return;
            }
            let handle = sockets.add(socket);
            debug!(
                "TCP socket {}: prepare for connection {} -> {}",
                handle, src, entry.listen_endpoint
            );
            entry.syn_queue.push_back(handle);
        }
    }
}

impl ListenTableEntry {
    fn matches_listener(
        &self,
        listen_endpoint: IpListenEndpoint,
        family: IpFamily,
        ipv6_only: bool,
    ) -> bool {
        self.listen_endpoint == listen_endpoint
            && self.family == family
            && self.ipv6_only == ipv6_only
    }
}

fn listeners_overlap(
    left_endpoint: IpListenEndpoint,
    left_family: IpFamily,
    left_ipv6_only: bool,
    right_endpoint: IpListenEndpoint,
    right_family: IpFamily,
    right_ipv6_only: bool,
) -> bool {
    if left_family == right_family {
        return left_endpoint.addr.is_none()
            || right_endpoint.addr.is_none()
            || left_endpoint.addr == right_endpoint.addr;
    }

    let (ipv6_endpoint, ipv6_only) = if left_family == IpFamily::Ipv6 {
        (left_endpoint, left_ipv6_only)
    } else {
        (right_endpoint, right_ipv6_only)
    };
    ipv6_endpoint.addr.is_none() && !ipv6_only
}

fn is_connected(handle: SocketHandle) -> bool {
    SOCKET_SET.with_socket::<tcp::Socket, _, _>(handle, |socket| {
        !matches!(socket.state(), State::Listen | State::SynReceived)
    })
}

fn is_closed(handle: SocketHandle) -> bool {
    SOCKET_SET
        .with_socket::<tcp::Socket, _, _>(handle, |socket| matches!(socket.state(), State::Closed))
}
