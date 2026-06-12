use alloc::vec;
use core::task::Waker;

use axpoll::PollSet;
use smoltcp::{
    storage::{PacketBuffer, PacketMetadata},
    time::Instant,
    wire::IpAddress,
};

use crate::{
    consts::{LOOPBACK_MTU, ROUTER_PACKET_BUFFER_SIZE},
    device::Device,
};

pub struct LoopbackDevice {
    buffer: PacketBuffer<'static, ()>,
    poll: PollSet,
}
impl LoopbackDevice {
    pub fn new() -> Self {
        let buffer = PacketBuffer::new(
            vec![PacketMetadata::EMPTY; ROUTER_PACKET_BUFFER_SIZE],
            vec![0u8; LOOPBACK_MTU * ROUTER_PACKET_BUFFER_SIZE],
        );
        Self {
            buffer,
            poll: PollSet::new(),
        }
    }
}

impl Device for LoopbackDevice {
    fn name(&self) -> &str {
        "lo"
    }

    fn is_loopback(&self) -> bool {
        true
    }

    fn notify_rx_ready(&self) {
        self.poll.wake();
    }

    fn recv(&mut self, buffer: &mut PacketBuffer<()>, _timestamp: Instant) -> bool {
        self.buffer.dequeue().ok().is_some_and(|(_, rx_buf)| {
            match buffer.enqueue(rx_buf.len(), ()) {
                Ok(buf) => {
                    buf.copy_from_slice(rx_buf);
                    true
                }
                Err(_) => false,
            }
        })
    }

    fn send(&mut self, next_hop: IpAddress, packet: &[u8], _timestamp: Instant) -> bool {
        match self.buffer.enqueue(packet.len(), ()) {
            Ok(tx_buf) => {
                tx_buf.copy_from_slice(packet);
                self.poll.wake();
                true
            }
            Err(_) => {
                warn!(
                    "Loopback device buffer is full, dropping packet to {}",
                    next_hop
                );
                false
            }
        }
    }

    fn register_waker(&self, waker: &Waker) {
        self.poll.register(waker);
    }
}
