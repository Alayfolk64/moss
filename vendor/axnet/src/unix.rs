pub(crate) mod dgram;
pub(crate) mod stream;

use alloc::{boxed::Box, sync::Arc};
use core::{
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::Context,
};

use async_trait::async_trait;
use axerrno::{AxError, AxResult};
use axfs::{FS_CONTEXT, OpenOptions};
use axfs_ng_vfs::NodeType;
use axio::{IoBuf, Read, Write};
use axpoll::{IoEvents, Pollable};
use axsync::Mutex;
use axtask::future::{block_on, interruptible};
use enum_dispatch::enum_dispatch;
use hashbrown::HashMap;
use lazy_static::lazy_static;

pub use self::{dgram::DgramTransport, stream::StreamTransport};
use crate::{
    RecvOptions, SendOptions, Shutdown, Socket, SocketAddrEx, SocketOps,
    options::{Configurable, GetSocketOption, SetSocketOption},
};

#[derive(Default, Clone, Debug, Hash, PartialEq, Eq)]
pub enum UnixSocketAddr {
    #[default]
    Unnamed,
    Abstract(Arc<[u8]>),
    Path(Arc<str>),
}

/// Abstract transport trait for Unix sockets.
#[async_trait]
#[enum_dispatch]
pub trait TransportOps: Configurable + Pollable + Send + Sync {
    fn bind(&self, slot: &BindSlot, local_addr: &UnixSocketAddr) -> AxResult;
    fn connect(&self, slot: &BindSlot, local_addr: &UnixSocketAddr) -> AxResult;

    async fn accept(&self) -> AxResult<(Transport, UnixSocketAddr)>;

    fn send(&self, src: impl Read + IoBuf, options: SendOptions) -> AxResult<usize>;
    fn recv(&self, dst: impl Write, options: RecvOptions<'_>) -> AxResult<usize>;

    fn shutdown(&self, _how: Shutdown) -> AxResult {
        Ok(())
    }
}

#[enum_dispatch(Configurable, TransportOps)]
pub enum Transport {
    Stream(StreamTransport),
    Dgram(DgramTransport),
}
impl Pollable for Transport {
    fn poll(&self) -> IoEvents {
        match self {
            Transport::Stream(stream) => stream.poll(),
            Transport::Dgram(dgram) => dgram.poll(),
        }
    }

    fn register(&self, context: &mut core::task::Context<'_>, events: IoEvents) {
        match self {
            Transport::Stream(stream) => stream.register(context, events),
            Transport::Dgram(dgram) => dgram.register(context, events),
        }
    }
}

#[derive(Default)]
pub struct BindSlot {
    stream: Mutex<Option<stream::Bind>>,
    dgram: Mutex<Option<dgram::Bind>>,
}

lazy_static! {
    static ref ABSTRACT_BINDS: Mutex<HashMap<Arc<[u8]>, BindSlot>> = Mutex::new(HashMap::new());
    // `/dev` is a synthetic device filesystem and cannot host a normal VFS
    // inode, but glibc and logging daemons use the conventional `/dev/log`
    // pathname UNIX datagram socket.  Keep that endpoint in the same socket
    // registry while preserving normal VFS semantics for all other paths.
    static ref DEV_LOG_BIND: Mutex<Option<BindSlot>> = Mutex::new(None);
    static ref UNIX_DIAGNOSTICS: Mutex<HashMap<(UnixSocketAddr, u8), u32>> =
        Mutex::new(HashMap::new());
}

static NEXT_UNIX_DIAGNOSTIC_INODE: AtomicU32 = AtomicU32::new(1);

pub(crate) struct UnixDiagnostic {
    pub address: UnixSocketAddr,
    pub socket_type: u8,
    pub state: u8,
    pub inode: u32,
}

pub(crate) fn unix_diagnostics() -> alloc::vec::Vec<UnixDiagnostic> {
    UNIX_DIAGNOSTICS
        .lock()
        .iter()
        .map(|((address, socket_type), inode)| UnixDiagnostic {
            address: address.clone(),
            socket_type: *socket_type,
            state: if *socket_type == 1 { 10 } else { 7 },
            inode: *inode,
        })
        .collect()
}

fn transport_socket_type(transport: &Transport) -> u8 {
    match transport {
        Transport::Stream(_) => 1,
        Transport::Dgram(_) => 2,
    }
}

fn register_diagnostic(address: &UnixSocketAddr, transport: &Transport) {
    let socket_type = transport_socket_type(transport);
    let inode = NEXT_UNIX_DIAGNOSTIC_INODE.fetch_add(1, Ordering::Relaxed);
    UNIX_DIAGNOSTICS
        .lock()
        .insert((address.clone(), socket_type), inode);
}

fn unregister_diagnostic(address: &UnixSocketAddr, transport: &Transport) {
    UNIX_DIAGNOSTICS
        .lock()
        .remove(&(address.clone(), transport_socket_type(transport)));
}

fn with_dev_log_slot<R>(
    path: &str,
    insert: bool,
    f: impl FnOnce(&BindSlot) -> AxResult<R>,
) -> AxResult<R> {
    if path != "/dev/log" {
        return Err(AxError::NotFound);
    }
    let mut slot = DEV_LOG_BIND.lock();
    if insert && slot.is_none() {
        *slot = Some(BindSlot::default());
    }
    f(slot.as_ref().ok_or(AxError::NotFound)?)
}

pub(crate) fn with_slot<R>(
    addr: &UnixSocketAddr,
    f: impl FnOnce(&BindSlot) -> AxResult<R>,
) -> AxResult<R> {
    match addr {
        UnixSocketAddr::Unnamed => Err(AxError::InvalidInput),
        UnixSocketAddr::Abstract(name) => {
            let binds = ABSTRACT_BINDS.lock();
            if let Some(slot) = binds.get(name) {
                f(slot)
            } else {
                Err(AxError::NotFound)
            }
        }
        UnixSocketAddr::Path(path) => {
            if path.as_ref() == "/dev/log" {
                return with_dev_log_slot(path, false, f);
            }
            let loc = FS_CONTEXT.lock().resolve(path.as_ref())?;
            if loc.metadata()?.node_type != NodeType::Socket {
                return Err(AxError::NotASocket);
            }
            f(loc
                .user_data()
                .get::<BindSlot>()
                .ok_or(AxError::ConnectionRefused)?
                .as_ref())
        }
    }
}

fn clear_slot(slot: &BindSlot, transport: &Transport) {
    match transport {
        Transport::Stream(_) => {
            *slot.stream.lock() = None;
        }
        Transport::Dgram(_) => {
            *slot.dgram.lock() = None;
        }
    }
}

fn is_slot_empty(slot: &BindSlot) -> bool {
    slot.stream.lock().is_none() && slot.dgram.lock().is_none()
}

fn cleanup_bound_slot(addr: &UnixSocketAddr, transport: &Transport) {
    match addr {
        UnixSocketAddr::Unnamed => {}
        UnixSocketAddr::Abstract(name) => {
            let mut binds = ABSTRACT_BINDS.lock();
            let should_remove = if let Some(slot) = binds.get(name) {
                clear_slot(slot, transport);
                is_slot_empty(slot)
            } else {
                false
            };
            if should_remove {
                binds.remove(name);
            }
        }
        UnixSocketAddr::Path(path) => {
            if path.as_ref() == "/dev/log" {
                let should_remove = {
                    let slot = DEV_LOG_BIND.lock();
                    if let Some(slot) = slot.as_ref() {
                        clear_slot(slot, transport);
                        is_slot_empty(slot)
                    } else {
                        false
                    }
                };
                if should_remove {
                    *DEV_LOG_BIND.lock() = None;
                }
                return;
            }
            let Ok(loc) = FS_CONTEXT.lock().resolve(path.as_ref()) else {
                return;
            };
            if !loc
                .metadata()
                .is_ok_and(|meta| meta.node_type == NodeType::Socket)
            {
                return;
            }
            let Some(slot) = loc.user_data().get::<BindSlot>() else {
                return;
            };
            clear_slot(slot.as_ref(), transport);
        }
    }
}
fn with_slot_or_insert<R>(
    addr: &UnixSocketAddr,
    f: impl FnOnce(&BindSlot) -> AxResult<R>,
) -> AxResult<R> {
    match addr {
        UnixSocketAddr::Unnamed => Err(AxError::InvalidInput),
        UnixSocketAddr::Abstract(name) => {
            let mut binds = ABSTRACT_BINDS.lock();
            f(binds.entry(name.clone()).or_default())
        }
        UnixSocketAddr::Path(path) => {
            if path.as_ref() == "/dev/log" {
                return with_dev_log_slot(path, true, f);
            }
            let loc = OpenOptions::new()
                .write(true)
                .create(true)
                .node_type(NodeType::Socket)
                .open(&FS_CONTEXT.lock(), path.as_ref())?
                .into_location();
            if loc.metadata()?.node_type != NodeType::Socket {
                return Err(AxError::NotASocket);
            }
            f(loc
                .user_data()
                .get_or_insert_with(BindSlot::default)
                .as_ref())
        }
    }
}

pub struct UnixSocket {
    transport: Transport,
    local_addr: Mutex<UnixSocketAddr>,
    remote_addr: Mutex<UnixSocketAddr>,
    has_bind_ownership: AtomicBool,
}
impl UnixSocket {
    pub fn new(transport: impl Into<Transport>) -> Self {
        Self {
            transport: transport.into(),
            local_addr: Mutex::new(UnixSocketAddr::Unnamed),
            remote_addr: Mutex::new(UnixSocketAddr::Unnamed),
            has_bind_ownership: AtomicBool::new(false),
        }
    }
}
impl Configurable for UnixSocket {
    fn get_option_inner(&self, opt: &mut GetSocketOption) -> AxResult<bool> {
        self.transport.get_option_inner(opt)
    }

    fn set_option_inner(&self, opt: SetSocketOption) -> AxResult<bool> {
        self.transport.set_option_inner(opt)
    }
}
impl SocketOps for UnixSocket {
    fn bind(&self, local_addr: SocketAddrEx) -> AxResult {
        let local_addr = local_addr.into_unix()?;
        let mut guard = self.local_addr.lock();
        if matches!(&*guard, UnixSocketAddr::Unnamed) {
            with_slot_or_insert(&local_addr, |slot| self.transport.bind(slot, &local_addr))?;
            *guard = local_addr;
            register_diagnostic(&guard, &self.transport);
            self.has_bind_ownership.store(true, Ordering::Release);
        } else {
            return Err(AxError::InvalidInput);
        }
        Ok(())
    }

    fn connect(&self, remote_addr: SocketAddrEx) -> AxResult {
        let remote_addr = remote_addr.into_unix()?;
        let local_addr = self.local_addr.lock().clone();
        let mut guard = self.remote_addr.lock();
        if matches!(&*guard, UnixSocketAddr::Unnamed) {
            with_slot(&remote_addr, |slot| {
                self.transport.connect(slot, &local_addr)
            })?;
            *guard = remote_addr;
        } else {
            return Err(AxError::InvalidInput);
        }
        Ok(())
    }

    fn listen(&self) -> AxResult {
        Ok(())
    }

    fn accept(&self) -> AxResult<Socket> {
        let (transport, peer_addr) = block_on(interruptible(self.transport.accept()))??;
        Ok(Socket::Unix(Self {
            transport,
            local_addr: Mutex::new(self.local_addr.lock().clone()),
            remote_addr: Mutex::new(peer_addr),
            has_bind_ownership: AtomicBool::new(false),
        }))
    }

    fn send(&self, src: impl Read + IoBuf, options: SendOptions) -> AxResult<usize> {
        self.transport.send(src, options)
    }

    fn recv(&self, dst: impl Write, options: RecvOptions<'_>) -> AxResult<usize> {
        self.transport.recv(dst, options)
    }

    fn local_addr(&self) -> AxResult<SocketAddrEx> {
        Ok(SocketAddrEx::Unix(self.local_addr.lock().clone()))
    }

    fn peer_addr(&self) -> AxResult<SocketAddrEx> {
        Ok(SocketAddrEx::Unix(self.remote_addr.lock().clone()))
    }

    fn shutdown(&self, how: Shutdown) -> AxResult {
        self.transport.shutdown(how)
    }
}

impl Pollable for UnixSocket {
    fn poll(&self) -> IoEvents {
        self.transport.poll()
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        self.transport.register(context, events);
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        if !self.has_bind_ownership.load(Ordering::Acquire) {
            return;
        }
        let local_addr = self.local_addr.lock().clone();
        unregister_diagnostic(&local_addr, &self.transport);
        cleanup_bound_slot(&local_addr, &self.transport);
    }
}
