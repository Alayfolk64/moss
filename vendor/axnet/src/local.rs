use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::{
    cmp::min,
    sync::atomic::{AtomicBool, Ordering},
};

use axerrno::{AxError, AxResult};
use axio::{IoBuf, IoBufMut, Read, Write};
use axpoll::{IoEvents, PollSet};
use axsync::Mutex;
use hashbrown::HashMap;
use lazyinit::LazyInit;
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint};

use crate::{IpFamily, RecvFlags, RecvOptions, Shutdown, SocketAddrEx, consts::LISTEN_QUEUE_SIZE};

static LOCAL_TRANSPORT: LazyInit<LocalTransport> = LazyInit::new();

const UNSPEC_V4: IpAddress = IpAddress::Ipv4(smoltcp::wire::Ipv4Address::UNSPECIFIED);
const UNSPEC_V6: IpAddress = IpAddress::Ipv6(smoltcp::wire::Ipv6Address::UNSPECIFIED);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct EndpointKey {
    addr: IpAddress,
    port: u16,
}

impl EndpointKey {
    fn new(addr: Option<IpAddress>, port: u16, family: IpFamily) -> Self {
        Self {
            addr: addr.unwrap_or_else(|| unspecified_for(family)),
            port,
        }
    }
}

fn unspecified_for(family: IpFamily) -> IpAddress {
    match family {
        IpFamily::Ipv4 => UNSPEC_V4,
        IpFamily::Ipv6 => UNSPEC_V6,
    }
}

fn other_unspecified_for(family: IpFamily) -> IpAddress {
    match family {
        IpFamily::Ipv4 => UNSPEC_V6,
        IpFamily::Ipv6 => UNSPEC_V4,
    }
}

fn keys_for_lookup(addr: IpAddress, port: u16, family: IpFamily) -> [EndpointKey; 3] {
    [
        EndpointKey { addr, port },
        EndpointKey {
            addr: unspecified_for(family),
            port,
        },
        EndpointKey {
            addr: other_unspecified_for(family),
            port,
        },
    ]
}

fn endpoint_accepts(
    listen: IpListenEndpoint,
    family: IpFamily,
    ipv6_only: bool,
    dst: IpEndpoint,
) -> bool {
    if listen.port != dst.port {
        return false;
    }
    match (listen.addr, dst.addr) {
        (Some(addr), dst_addr) => addr == dst_addr,
        (None, IpAddress::Ipv4(_)) => family == IpFamily::Ipv4 || !ipv6_only,
        (None, IpAddress::Ipv6(_)) => family == IpFamily::Ipv6,
    }
}

fn wildcard_accepts_v4(family: IpFamily, ipv6_only: bool) -> bool {
    family == IpFamily::Ipv4 || !ipv6_only
}

fn wildcard_accepts_v6(family: IpFamily) -> bool {
    family == IpFamily::Ipv6
}

fn tcp_listeners_conflict(
    current: &LocalTcpListener,
    endpoint: IpListenEndpoint,
    family: IpFamily,
    ipv6_only: bool,
) -> bool {
    if current.endpoint.port != endpoint.port {
        return false;
    }

    match (current.endpoint.addr, endpoint.addr) {
        (Some(current_addr), Some(addr)) => current_addr == addr,
        (None, Some(addr)) => current.accepts(IpEndpoint {
            addr,
            port: endpoint.port,
        }),
        (Some(current_addr), None) => endpoint_accepts(
            endpoint,
            family,
            ipv6_only,
            IpEndpoint {
                addr: current_addr,
                port: endpoint.port,
            },
        ),
        (None, None) => {
            (wildcard_accepts_v4(current.family, current.ipv6_only)
                && wildcard_accepts_v4(family, ipv6_only))
                || (wildcard_accepts_v6(current.family) && wildcard_accepts_v6(family))
        }
    }
}

pub(crate) fn init() {
    LOCAL_TRANSPORT.init_once(LocalTransport::new());
}

fn local_transport() -> &'static LocalTransport {
    &LOCAL_TRANSPORT
}

pub(crate) struct UdpPacket {
    data: Vec<u8>,
    remote_endpoint: IpEndpoint,
    local_address: IpAddress,
}

pub(crate) struct LocalUdpSocket {
    family: IpFamily,
    ipv6_only: AtomicBool,
    endpoint: Mutex<Option<IpListenEndpoint>>,
    connected: Mutex<Option<IpEndpoint>>,
    rx: Mutex<VecDeque<UdpPacket>>,
    poll_state: Arc<PollSet>,
}

impl LocalUdpSocket {
    pub(crate) fn new(family: IpFamily) -> Arc<Self> {
        Arc::new(Self {
            family,
            ipv6_only: AtomicBool::new(false),
            endpoint: Mutex::new(None),
            connected: Mutex::new(None),
            rx: Mutex::new(VecDeque::with_capacity(LISTEN_QUEUE_SIZE)),
            poll_state: Arc::new(PollSet::new()),
        })
    }

    pub(crate) fn set_ipv6_only(&self, ipv6_only: bool) {
        self.ipv6_only.store(ipv6_only, Ordering::Relaxed);
    }

    pub(crate) fn bind(self: &Arc<Self>, endpoint: IpListenEndpoint) -> AxResult {
        let mut guard = self.endpoint.lock();
        if guard.is_some() {
            return Err(AxError::InvalidInput);
        }
        local_transport().bind_udp(endpoint, self.family, self)?;
        *guard = Some(endpoint);
        Ok(())
    }

    pub(crate) fn connect(&self, remote: IpEndpoint) {
        *self.connected.lock() = Some(remote);
        self.poll_state.wake();
    }

    pub(crate) fn send<S>(
        &self,
        src: S,
        remote_addr: IpEndpoint,
        source_addr: IpAddress,
    ) -> AxResult<Result<usize, S>>
    where
        S: Read + IoBuf,
    {
        if !remote_addr.addr.is_loopback() {
            return Ok(Err(src));
        }

        let local_port = self.endpoint.lock().map_or(0, |endpoint| endpoint.port);
        if local_port == 0 {
            return Ok(Err(src));
        }

        let remote_endpoint = IpEndpoint {
            addr: source_addr,
            port: local_port,
        };
        local_transport().send_udp(
            remote_addr,
            remote_endpoint,
            remote_addr.addr,
            src,
            self.family,
        )
    }

    fn check_incoming(&self, dst: IpEndpoint, remote_endpoint: IpEndpoint) -> AxResult {
        if !self.accepts_endpoint(dst) {
            return Err(AxError::ConnectionRefused);
        }
        if let Some(peer) = *self.connected.lock() {
            if peer != remote_endpoint {
                return Err(AxError::ConnectionRefused);
            }
        }
        Ok(())
    }

    fn accepts_endpoint(&self, dst: IpEndpoint) -> bool {
        let Some(endpoint) = *self.endpoint.lock() else {
            return false;
        };
        endpoint_accepts(
            endpoint,
            self.family,
            self.ipv6_only.load(Ordering::Relaxed),
            dst,
        )
    }

    fn connected_peer(&self) -> Option<IpEndpoint> {
        *self.connected.lock()
    }

    fn enqueue_from_reader<S>(
        &self,
        dst: IpEndpoint,
        remote_endpoint: IpEndpoint,
        local_address: IpAddress,
        mut src: S,
    ) -> AxResult<Result<usize, S>>
    where
        S: Read + IoBuf,
    {
        self.check_incoming(dst, remote_endpoint)?;
        let mut rx = self.rx.lock();
        if rx.len() >= LISTEN_QUEUE_SIZE {
            return Ok(Err(src));
        }
        let size = src.remaining();
        let mut data = vec![0; size];
        src.read_exact(&mut data)?;
        rx.push_back(UdpPacket {
            data,
            remote_endpoint,
            local_address,
        });
        drop(rx);
        self.poll_state.wake();
        Ok(Ok(size))
    }

    pub(crate) fn recv(
        &self,
        mut dst: impl Write,
        family: IpFamily,
        mut options: RecvOptions<'_>,
    ) -> AxResult<usize> {
        enum ExpectedRemote<'a> {
            Any(&'a mut SocketAddrEx),
            Expecting(IpEndpoint),
            Unspecified,
        }
        let mut expected_remote = match options.from.take() {
            Some(addr) => ExpectedRemote::Any(addr),
            None => match *self.connected.lock() {
                Some(endpoint) => ExpectedRemote::Expecting(endpoint),
                None => ExpectedRemote::Unspecified,
            },
        };

        let packet = {
            let mut rx = self.rx.lock();
            let idx = rx
                .iter()
                .position(|packet| match &expected_remote {
                    ExpectedRemote::Expecting(expected) => {
                        (expected.addr.is_unspecified()
                            || expected.addr == packet.remote_endpoint.addr)
                            && (expected.port == 0 || expected.port == packet.remote_endpoint.port)
                    }
                    _ => true,
                })
                .ok_or(AxError::WouldBlock)?;
            if options.flags.contains(RecvFlags::PEEK) {
                let packet = rx.get(idx).ok_or(AxError::WouldBlock)?;
                UdpPacket {
                    data: packet.data.clone(),
                    remote_endpoint: packet.remote_endpoint,
                    local_address: packet.local_address,
                }
            } else {
                rx.remove(idx).ok_or(AxError::WouldBlock)?
            }
        };

        match &mut expected_remote {
            ExpectedRemote::Any(remote_addr) => {
                **remote_addr =
                    SocketAddrEx::Ip(family.format_socket_addr(
                        packet.remote_endpoint.addr,
                        packet.remote_endpoint.port,
                    ));
            }
            ExpectedRemote::Expecting(_) | ExpectedRemote::Unspecified => {}
        }

        let len = packet.data.len();
        let read = dst.write(&packet.data)?;
        if read < len {
            warn!("UDP message truncated: {} -> {} bytes", len, read);
        }

        Ok(if options.flags.contains(RecvFlags::TRUNCATE) {
            len
        } else {
            read
        })
    }

    pub(crate) fn poll(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        events.set(IoEvents::IN, !self.rx.lock().is_empty());
        events.set(IoEvents::OUT, self.endpoint.lock().is_some());
        events
    }

    pub(crate) fn register(&self, waker: &core::task::Waker) {
        self.poll_state.register(waker);
    }

    pub(crate) fn close(&self) {
        if let Some(endpoint) = self.endpoint.lock().take() {
            local_transport().unbind_udp(endpoint, self.family, self);
        }
        self.rx.lock().clear();
        self.poll_state.wake();
    }
}

struct LocalTcpListener {
    endpoint: IpListenEndpoint,
    family: IpFamily,
    ipv6_only: bool,
    queue: Mutex<VecDeque<Arc<LocalTcpStream>>>,
    poll_state: Arc<PollSet>,
    rx_buf_len: usize,
    tx_buf_len: usize,
}

impl LocalTcpListener {
    fn accepts(&self, dst: IpEndpoint) -> bool {
        endpoint_accepts(self.endpoint, self.family, self.ipv6_only, dst)
    }

    fn enqueue(&self, stream: Arc<LocalTcpStream>) -> AxResult {
        let mut queue = self.queue.lock();
        if queue.len() >= LISTEN_QUEUE_SIZE {
            return Err(AxError::ConnectionRefused);
        }
        queue.push_back(stream);
        drop(queue);
        self.poll_state.wake();
        Ok(())
    }

    fn accept(&self) -> AxResult<Arc<LocalTcpStream>> {
        self.queue.lock().pop_front().ok_or(AxError::WouldBlock)
    }

    fn can_accept(&self) -> bool {
        !self.queue.lock().is_empty()
    }
}

pub(crate) struct LocalTcpSocket {
    family: IpFamily,
    bound: Mutex<Option<IpListenEndpoint>>,
    listener: Mutex<Option<Arc<LocalTcpListener>>>,
    stream: Mutex<Option<Arc<LocalTcpStream>>>,
    poll_state: Arc<PollSet>,
}

impl LocalTcpSocket {
    /// Initializes local TCP state directly inside its final socket allocation.
    ///
    /// # Safety
    ///
    /// `dst` must be valid, aligned, uninitialized storage for one
    /// `LocalTcpSocket`, and the outer socket must remain unpublished until
    /// initialization completes.
    pub(crate) unsafe fn write_unpublished_in_place(
        dst: *mut Self,
        family: IpFamily,
        bound: Option<IpListenEndpoint>,
        stream: Option<Arc<LocalTcpStream>>,
    ) {
        unsafe {
            core::ptr::addr_of_mut!((*dst).family).write(family);
            axsync::write_unpublished_in_place(core::ptr::addr_of_mut!((*dst).bound), bound);
            axsync::write_unpublished_in_place(
                core::ptr::addr_of_mut!((*dst).listener),
                None,
            );
            axsync::write_unpublished_in_place(core::ptr::addr_of_mut!((*dst).stream), stream);
            core::ptr::addr_of_mut!((*dst).poll_state).write(PollSet::new_shared());
        }
    }

    /// Repairs freshly constructed state in an accepted socket after the
    /// socket has reached its final allocation but before it is published.
    pub(crate) unsafe fn reinitialize_accepted_after_move(&mut self) -> [u64; 3] {
        let previous_owners = unsafe {
            let previous_owners = [
                axsync::reinitialize_unpublished(&mut self.bound),
                axsync::reinitialize_unpublished(&mut self.listener),
                axsync::reinitialize_unpublished(&mut self.stream),
            ];

            // An accepted socket can never itself be a listener. On the
            // pinned LoongArch toolchain the same aggregate move that has
            // polluted fresh RawMutex atomics has turned this `None` niche
            // into small task IDs. Do not assign here: that would first drop
            // the invalid Arc pointer. The socket is private, and the
            // constructor invariant makes overwriting the intended `None`
            // value without dropping it the only valid recovery.
            core::ptr::write(self.listener.get_mut(), None);
            previous_owners
        };
        previous_owners
    }

    pub(crate) fn bind(&self, endpoint: IpListenEndpoint) -> AxResult {
        let mut bound = self.bound.lock();
        if bound.is_some() {
            return Err(AxError::InvalidInput);
        }
        *bound = Some(endpoint);
        self.poll_state.wake();
        Ok(())
    }

    pub(crate) fn bound_endpoint(&self) -> Option<IpListenEndpoint> {
        *self.bound.lock()
    }

    pub(crate) fn reset_bound_if(&self, endpoint: IpListenEndpoint) {
        let mut bound = self.bound.lock();
        if *bound == Some(endpoint) {
            *bound = None;
            self.poll_state.wake();
        }
    }

    pub(crate) fn listen(
        &self,
        family: IpFamily,
        ipv6_only: bool,
        rx_buf_len: usize,
        tx_buf_len: usize,
    ) -> AxResult {
        let endpoint = self.bound_endpoint().ok_or(AxError::InvalidInput)?;
        let listener = local_transport().listen_tcp(
            endpoint,
            family,
            ipv6_only,
            rx_buf_len,
            tx_buf_len,
            self.poll_state.clone(),
        )?;
        *self.listener.lock() = Some(listener);
        self.poll_state.wake();
        Ok(())
    }

    pub(crate) fn connect(
        &self,
        mut local: IpListenEndpoint,
        remote: IpEndpoint,
        ipv6_only: bool,
        rx_buf_len: usize,
        tx_buf_len: usize,
    ) -> AxResult<bool> {
        if !remote.addr.is_loopback() {
            return Ok(false);
        }
        let Some(local_addr) = local.addr else {
            return Ok(false);
        };

        let stream = match local_transport().connect_tcp(
            local_addr,
            local.port,
            remote,
            self.family,
            ipv6_only,
            rx_buf_len,
            tx_buf_len,
        ) {
            Ok(stream) => stream,
            Err(AxError::ConnectionRefused) => return Err(AxError::ConnectionRefused),
            Err(err) => return Err(err),
        };

        local.addr = Some(local_addr);
        *self.bound.lock() = Some(local);
        *self.stream.lock() = Some(stream);
        self.poll_state.wake();
        Ok(true)
    }

    pub(crate) fn accept(&self) -> AxResult<Arc<LocalTcpStream>> {
        let listener = self.listener.lock().clone().ok_or(AxError::InvalidInput)?;
        listener.accept()
    }

    pub(crate) fn can_accept(&self) -> bool {
        self.listener
            .lock()
            .as_ref()
            .map_or(false, |listener| listener.can_accept())
    }

    pub(crate) fn send<R>(&self, src: &mut R) -> AxResult<Option<usize>>
    where
        R: Read + IoBuf,
    {
        let Some(stream) = self.stream.lock().clone() else {
            return Ok(None);
        };
        stream.send(src).map(Some)
    }

    pub(crate) fn recv<W>(&self, dst: &mut W, peek: bool) -> AxResult<Option<usize>>
    where
        W: Write + IoBufMut,
    {
        let Some(stream) = self.stream.lock().clone() else {
            return Ok(None);
        };
        stream.recv(dst, peek).map(Some)
    }

    pub(crate) fn local_endpoint(&self) -> Option<IpEndpoint> {
        if let Some(stream) = self.stream.lock().as_ref() {
            return Some(stream.local_endpoint());
        }
        self.bound.lock().map(|endpoint| IpEndpoint {
            addr: endpoint
                .addr
                .unwrap_or_else(|| unspecified_for(self.family)),
            port: endpoint.port,
        })
    }

    pub(crate) fn remote_endpoint(&self) -> Option<IpEndpoint> {
        self.stream
            .lock()
            .as_ref()
            .map(|stream| stream.remote_endpoint())
    }

    pub(crate) fn poll_stream(&self) -> Option<IoEvents> {
        self.stream.lock().as_ref().map(|stream| stream.poll())
    }

    pub(crate) fn register(&self, waker: &core::task::Waker) {
        let stream = self.stream.lock().clone();
        let listener = self.listener.lock().clone();

        if stream.is_none() && listener.is_none() {
            self.poll_state.register(waker);
        }
        if let Some(stream) = stream.as_ref() {
            stream.register(waker);
        }
        if let Some(listener) = listener.as_ref() {
            listener.poll_state.register(waker);
        }
    }

    pub(crate) fn shutdown(&self, how: Shutdown) {
        if let Some(stream) = self.stream.lock().as_ref() {
            stream.shutdown(how);
        }
        self.poll_state.wake();
    }

    pub(crate) fn close(&self) {
        if let Some(stream) = self.stream.lock().take() {
            stream.shutdown(Shutdown::Both);
        }
        if let Some(listener) = self.listener.lock().take() {
            local_transport().unlisten_tcp(listener.endpoint, listener.family);
        }
        self.poll_state.wake();
    }

    pub(crate) fn reset_listener(&self) {
        if let Some(listener) = self.listener.lock().take() {
            local_transport().unlisten_tcp(listener.endpoint, listener.family);
        }
        self.poll_state.wake();
    }
}

struct StreamBuffer {
    data: Vec<u8>,
    head: usize,
    len: usize,
    capacity: usize,
}

impl StreamBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: Vec::new(),
            head: 0,
            len: 0,
            capacity,
        }
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn is_full(&self) -> bool {
        self.len >= self.capacity
    }

    fn grow_to(&mut self, min_capacity: usize) {
        if self.data.len() >= min_capacity {
            return;
        }

        let mut new_capacity = self.data.len().max(1);
        while new_capacity < min_capacity {
            new_capacity = (new_capacity * 2).min(self.capacity);
        }

        let mut data = vec![0; new_capacity];
        if self.len != 0 {
            let (front, back) = self.as_slices();
            data[..front.len()].copy_from_slice(front);
            data[front.len()..front.len() + back.len()].copy_from_slice(back);
        }
        self.data = data;
        self.head = 0;
    }

    fn as_slices(&self) -> (&[u8], &[u8]) {
        if self.len == 0 || self.data.is_empty() {
            return (&[], &[]);
        }

        let first_len = min(self.len, self.data.len() - self.head);
        let second_len = self.len - first_len;
        (
            &self.data[self.head..self.head + first_len],
            &self.data[..second_len],
        )
    }

    fn write_from(&mut self, src: &mut (impl Read + IoBuf)) -> AxResult<usize> {
        let len = min(
            src.remaining(),
            self.capacity.saturating_sub(self.len),
        );
        if len == 0 {
            return Ok(0);
        }

        self.grow_to(self.len + len);
        if self.len == 0 {
            self.head = 0;
        }

        let tail = (self.head + self.len) % self.data.len();
        let contiguous = if self.len == 0 {
            self.data.len()
        } else if tail >= self.head {
            self.data.len() - tail
        } else {
            self.head - tail
        };
        let read = src.read(&mut self.data[tail..tail + min(len, contiguous)])?;
        self.len += read;
        Ok(read)
    }

    fn read_into(&mut self, dst: &mut (impl Write + IoBufMut), peek: bool) -> AxResult<usize> {
        let len = min(dst.remaining_mut(), self.len);
        if len == 0 {
            return Ok(0);
        }

        let (front, back) = self.as_slices();
        let mut count = dst.write(&front[..min(front.len(), len)])?;
        if count == front.len() && count < len && !dst.is_full() {
            count += dst.write(&back[..min(back.len(), len - count)])?;
        }
        if !peek {
            self.head = (self.head + count) % self.data.len();
            self.len -= count;
            if self.len == 0 {
                self.head = 0;
            }
        }
        Ok(count)
    }
}

pub(crate) struct LocalTcpStream {
    local: IpEndpoint,
    remote: IpEndpoint,
    rx: Mutex<StreamBuffer>,
    peer: Mutex<Weak<LocalTcpStream>>,
    rx_closed: AtomicBool,
    tx_closed: AtomicBool,
    peer_tx_closed: AtomicBool,
    poll_state: Arc<PollSet>,
}

impl LocalTcpStream {
    fn pair(
        client: IpEndpoint,
        server: IpEndpoint,
        client_rx_len: usize,
        client_tx_len: usize,
        server_rx_len: usize,
        server_tx_len: usize,
    ) -> (Arc<Self>, Arc<Self>) {
        let client_rx_capacity = min(server_tx_len, client_rx_len).max(1);
        let server_rx_capacity = min(client_tx_len, server_rx_len).max(1);

        let client_stream = Arc::new(Self {
            local: client,
            remote: server,
            rx: Mutex::new(StreamBuffer::new(client_rx_capacity)),
            peer: Mutex::new(Weak::new()),
            rx_closed: AtomicBool::new(false),
            tx_closed: AtomicBool::new(false),
            peer_tx_closed: AtomicBool::new(false),
            poll_state: Arc::new(PollSet::new()),
        });
        let server_stream = Arc::new(Self {
            local: server,
            remote: client,
            rx: Mutex::new(StreamBuffer::new(server_rx_capacity)),
            peer: Mutex::new(Arc::downgrade(&client_stream)),
            rx_closed: AtomicBool::new(false),
            tx_closed: AtomicBool::new(false),
            peer_tx_closed: AtomicBool::new(false),
            poll_state: Arc::new(PollSet::new()),
        });
        *client_stream.peer.lock() = Arc::downgrade(&server_stream);
        (client_stream, server_stream)
    }

    pub(crate) fn local_endpoint(&self) -> IpEndpoint {
        self.local
    }

    pub(crate) fn remote_endpoint(&self) -> IpEndpoint {
        self.remote
    }

    fn peer(&self) -> Option<Arc<Self>> {
        self.peer.lock().upgrade()
    }

    pub(crate) fn send(&self, mut src: impl Read + IoBuf) -> AxResult<usize> {
        if self.tx_closed.load(Ordering::Acquire) {
            return Err(AxError::BrokenPipe);
        }
        if src.remaining() == 0 {
            return Ok(0);
        }
        let Some(peer) = self.peer() else {
            return Err(AxError::BrokenPipe);
        };
        if peer.rx_closed.load(Ordering::Acquire) {
            return Err(AxError::BrokenPipe);
        }

        let mut rx = peer.rx.lock();
        if rx.is_full() {
            return Err(AxError::WouldBlock);
        }
        let len = rx.write_from(&mut src)?;
        drop(rx);

        if len > 0 {
            peer.poll_state.wake();
            Ok(len)
        } else {
            Ok(0)
        }
    }

    pub(crate) fn recv(&self, mut dst: impl Write + IoBufMut, peek: bool) -> AxResult<usize> {
        if self.rx_closed.load(Ordering::Acquire) {
            return Err(AxError::NotConnected);
        }
        let mut rx = self.rx.lock();
        if rx.is_empty() {
            if self.peer_tx_closed.load(Ordering::Acquire) {
                return Ok(0);
            }
            return Err(AxError::WouldBlock);
        }

        let len = rx.read_into(&mut dst, peek)?;
        drop(rx);

        if len > 0 {
            if let Some(peer) = self.peer() {
                peer.poll_state.wake();
            }
        }
        Ok(len)
    }

    pub(crate) fn poll(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        let may_recv = !self.rx_closed.load(Ordering::Acquire);
        let may_send = !self.tx_closed.load(Ordering::Acquire);

        let can_recv = !self.rx.lock().is_empty() || self.peer_tx_closed.load(Ordering::Acquire);
        let can_send = self.peer().map_or(true, |peer| {
            !peer.rx_closed.load(Ordering::Acquire) && !peer.rx.lock().is_full()
        });
        events.set(IoEvents::IN, may_recv && can_recv);
        events.set(IoEvents::OUT, may_send && can_send);
        events.set(IoEvents::RDHUP, self.peer_tx_closed.load(Ordering::Acquire));
        events
    }

    pub(crate) fn register(&self, waker: &core::task::Waker) {
        self.poll_state.register(waker);
    }

    pub(crate) fn shutdown(&self, how: Shutdown) {
        if how.has_read() {
            self.rx_closed.store(true, Ordering::Release);
        }
        if how.has_write() && !self.tx_closed.swap(true, Ordering::AcqRel) {
            if let Some(peer) = self.peer() {
                peer.peer_tx_closed.store(true, Ordering::Release);
                peer.poll_state.wake();
            }
        }
        self.poll_state.wake();
    }
}

impl Drop for LocalTcpStream {
    fn drop(&mut self) {
        if let Some(peer) = self.peer() {
            peer.peer_tx_closed.store(true, Ordering::Release);
            peer.poll_state.wake();
        }
    }
}

struct LocalTransport {
    // UDP reuse needs multiple sockets on one endpoint: iperf3 keeps a listener
    // and creates per-peer connected stream sockets on the same server port.
    udp: Mutex<HashMap<EndpointKey, Vec<Weak<LocalUdpSocket>>>>,
    tcp: Mutex<HashMap<EndpointKey, Arc<LocalTcpListener>>>,
}

impl LocalTransport {
    fn new() -> Self {
        Self {
            udp: Mutex::new(HashMap::new()),
            tcp: Mutex::new(HashMap::new()),
        }
    }

    fn bind_udp(
        &self,
        endpoint: IpListenEndpoint,
        family: IpFamily,
        socket: &Arc<LocalUdpSocket>,
    ) -> AxResult {
        let key = EndpointKey::new(endpoint.addr, endpoint.port, family);
        let mut udp = self.udp.lock();
        udp.entry(key).or_default().push(Arc::downgrade(socket));
        Ok(())
    }

    fn unbind_udp(&self, endpoint: IpListenEndpoint, family: IpFamily, socket: &LocalUdpSocket) {
        let key = EndpointKey::new(endpoint.addr, endpoint.port, family);
        let socket_ptr = socket as *const LocalUdpSocket;
        let mut udp = self.udp.lock();
        if let Some(sockets) = udp.get_mut(&key) {
            sockets.retain(|weak| {
                weak.upgrade()
                    .map_or(false, |registered| Arc::as_ptr(&registered) != socket_ptr)
            });
            if sockets.is_empty() {
                udp.remove(&key);
            }
        }
    }

    fn lookup_udp(
        &self,
        endpoint: IpEndpoint,
        remote_endpoint: IpEndpoint,
        family: IpFamily,
    ) -> Option<Arc<LocalUdpSocket>> {
        let candidates = {
            let mut udp = self.udp.lock();
            let mut candidates = Vec::new();
            let mut stale = Vec::new();

            for key in keys_for_lookup(endpoint.addr, endpoint.port, family) {
                if let Some(sockets) = udp.get_mut(&key) {
                    sockets.retain(|weak| {
                        if let Some(socket) = weak.upgrade() {
                            candidates.push(socket);
                            true
                        } else {
                            false
                        }
                    });
                    if sockets.is_empty() {
                        stale.push(key);
                    }
                }
            }
            for key in stale {
                udp.remove(&key);
            }
            candidates
        };

        let mut listener = None;
        for socket in candidates {
            if !socket.accepts_endpoint(endpoint) {
                continue;
            }
            match socket.connected_peer() {
                Some(peer) if peer == remote_endpoint => return Some(socket),
                Some(_) => {}
                None if listener.is_none() => listener = Some(socket),
                None => {}
            }
        }
        listener
    }

    fn send_udp<S>(
        &self,
        dst: IpEndpoint,
        remote_endpoint: IpEndpoint,
        local_address: IpAddress,
        src: S,
        family: IpFamily,
    ) -> AxResult<Result<usize, S>>
    where
        S: Read + IoBuf,
    {
        let Some(socket) = self.lookup_udp(dst, remote_endpoint, family) else {
            return Ok(Err(src));
        };
        socket.enqueue_from_reader(dst, remote_endpoint, local_address, src)
    }

    fn listen_tcp(
        &self,
        endpoint: IpListenEndpoint,
        family: IpFamily,
        ipv6_only: bool,
        rx_buf_len: usize,
        tx_buf_len: usize,
        poll_state: Arc<PollSet>,
    ) -> AxResult<Arc<LocalTcpListener>> {
        let key = EndpointKey::new(endpoint.addr, endpoint.port, family);
        let mut tcp = self.tcp.lock();
        if tcp
            .values()
            .any(|listener| tcp_listeners_conflict(listener, endpoint, family, ipv6_only))
        {
            return Err(AxError::AddrInUse);
        }
        let listener = Arc::new(LocalTcpListener {
            endpoint,
            family,
            ipv6_only,
            queue: Mutex::new(VecDeque::with_capacity(LISTEN_QUEUE_SIZE)),
            poll_state,
            rx_buf_len,
            tx_buf_len,
        });
        tcp.insert(key, listener.clone());
        Ok(listener)
    }

    fn unlisten_tcp(&self, endpoint: IpListenEndpoint, family: IpFamily) {
        self.tcp
            .lock()
            .remove(&EndpointKey::new(endpoint.addr, endpoint.port, family));
    }

    fn lookup_tcp(&self, endpoint: IpEndpoint, family: IpFamily) -> Option<Arc<LocalTcpListener>> {
        let candidates = {
            let tcp = self.tcp.lock();
            let mut candidates = Vec::new();
            for key in keys_for_lookup(endpoint.addr, endpoint.port, family) {
                if let Some(listener) = tcp.get(&key) {
                    candidates.push(listener.clone());
                }
            }
            candidates
        };

        for listener in candidates {
            if listener.accepts(endpoint) {
                return Some(listener);
            }
        }
        None
    }

    fn connect_tcp(
        &self,
        local_addr: IpAddress,
        local_port: u16,
        remote: IpEndpoint,
        family: IpFamily,
        _ipv6_only: bool,
        rx_buf_len: usize,
        tx_buf_len: usize,
    ) -> AxResult<Arc<LocalTcpStream>> {
        let listener = self
            .lookup_tcp(remote, family)
            .ok_or(AxError::ConnectionRefused)?;
        let local = IpEndpoint {
            addr: local_addr,
            port: local_port,
        };
        let (client, server) = LocalTcpStream::pair(
            local,
            remote,
            rx_buf_len,
            tx_buf_len,
            listener.rx_buf_len,
            listener.tx_buf_len,
        );
        listener.enqueue(server)?;
        Ok(client)
    }
}
