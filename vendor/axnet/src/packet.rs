use core::{
    sync::atomic::{AtomicU32, Ordering},
    task::Context,
};

use axerrno::{AxError, AxResult};
use axio::prelude::*;
use axpoll::{IoEvents, Pollable};

use crate::{
    RecvOptions, SendOptions, Shutdown, SocketAddrEx, SocketOps,
    general::GeneralOptions,
    options::{Configurable, GetSocketOption, SetSocketOption},
};

const TPACKET_V3: u32 = 2;

#[derive(Clone, Copy, Debug, Default)]
pub struct PacketRxRing {
    pub block_size: u32,
    pub block_nr: u32,
    pub frame_size: u32,
    pub frame_nr: u32,
    pub sizeof_priv: u32,
}

pub struct PacketSocket {
    protocol: u32,
    version: AtomicU32,
    reserve: AtomicU32,
    ring_block_size: AtomicU32,
    general: GeneralOptions,
}

impl PacketSocket {
    pub fn new(protocol: u32) -> Self {
        Self {
            protocol,
            version: AtomicU32::new(0),
            reserve: AtomicU32::new(0),
            ring_block_size: AtomicU32::new(0),
            general: GeneralOptions::new(),
        }
    }

    pub fn set_packet_version(&self, version: u32) -> AxResult {
        if version > TPACKET_V3 {
            return Err(AxError::InvalidInput);
        }
        self.version.store(version, Ordering::Relaxed);
        Ok(())
    }

    pub fn packet_version(&self) -> u32 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn set_packet_reserve(&self, reserve: u32) -> AxResult {
        let ring_block_size = self.ring_block_size.load(Ordering::Acquire);
        if ring_block_size != 0 && reserve > ring_block_size {
            return Err(AxError::InvalidInput);
        }
        self.reserve.store(reserve, Ordering::Release);
        Ok(())
    }

    pub fn packet_reserve(&self) -> u32 {
        self.reserve.load(Ordering::Acquire)
    }

    pub fn set_rx_ring(&self, req: PacketRxRing) -> AxResult {
        if req.block_nr == 0 && req.frame_nr == 0 {
            self.ring_block_size.store(0, Ordering::Release);
            return Ok(());
        }

        if req.block_size == 0 || req.block_nr == 0 || req.frame_size == 0 || req.frame_nr == 0 {
            return Err(AxError::InvalidInput);
        }
        if req.frame_size > req.block_size || req.block_size % req.frame_size != 0 {
            return Err(AxError::InvalidInput);
        }
        let frames_per_block = req.block_size / req.frame_size;
        let Some(frame_nr) = frames_per_block.checked_mul(req.block_nr) else {
            return Err(AxError::InvalidInput);
        };
        if frame_nr != req.frame_nr {
            return Err(AxError::InvalidInput);
        }
        if req.sizeof_priv > req.block_size {
            return Err(AxError::InvalidInput);
        }

        let reserve = self.reserve.load(Ordering::Acquire);
        if reserve > req.block_size {
            self.reserve.store(req.block_size, Ordering::Release);
        }
        self.ring_block_size
            .store(req.block_size, Ordering::Release);
        Ok(())
    }
}

impl Configurable for PacketSocket {
    fn get_option_inner(&self, option: &mut GetSocketOption) -> AxResult<bool> {
        self.general.get_option_inner(option)
    }

    fn set_option_inner(&self, option: SetSocketOption) -> AxResult<bool> {
        self.general.set_option_inner(option)
    }
}

impl SocketOps for PacketSocket {
    fn bind(&self, _local_addr: SocketAddrEx) -> AxResult {
        Err(AxError::OperationNotSupported)
    }

    fn connect(&self, _remote_addr: SocketAddrEx) -> AxResult {
        Err(AxError::OperationNotSupported)
    }

    fn send(&self, _src: impl Read + IoBuf, _options: SendOptions) -> AxResult<usize> {
        Err(AxError::OperationNotSupported)
    }

    fn recv(&self, _dst: impl Write + IoBufMut, _options: RecvOptions<'_>) -> AxResult<usize> {
        Err(AxError::WouldBlock)
    }

    fn local_addr(&self) -> AxResult<SocketAddrEx> {
        Err(AxError::OperationNotSupported)
    }

    fn peer_addr(&self) -> AxResult<SocketAddrEx> {
        Err(AxError::OperationNotSupported)
    }

    fn shutdown(&self, _how: Shutdown) -> AxResult {
        Ok(())
    }
}

impl Pollable for PacketSocket {
    fn poll(&self) -> IoEvents {
        let _ = self.protocol;
        IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}
