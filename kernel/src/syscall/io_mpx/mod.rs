mod epoll;
mod poll;
mod select;

use alloc::{sync::Arc, vec::Vec};
use core::task::Context;

use axpoll::{IoEvents, Pollable};

pub use self::{epoll::*, poll::*, select::*};
use crate::file::FileLike;

fn poll_network_once() {
    #[cfg(feature = "net")]
    axnet::poll_interfaces();
}

struct FdPollSet(pub Vec<(Arc<dyn FileLike>, IoEvents)>);
impl Pollable for FdPollSet {
    fn poll(&self) -> IoEvents {
        unreachable!()
    }

    fn register(&self, context: &mut Context<'_>, _events: IoEvents) {
        for (file, events) in &self.0 {
            file.register(context, *events);
        }
    }
}
