mod fs;
mod inode;
mod util;

use alloc::boxed::Box;

#[allow(unused_imports)]
use axdriver::{AxBlockDevice, prelude::BlockDriverOps};
pub use fs::*;
pub use inode::*;
use lwext4_rust::{BlockDevice, Ext4Error, Ext4Result, ffi::EIO};

pub(crate) enum Ext4Disk {
    Runtime(AxBlockDevice),
    Driver(Box<dyn BlockDriverOps>),
}

impl Ext4Disk {
    pub(crate) fn runtime(dev: AxBlockDevice) -> Self {
        Self::Runtime(dev)
    }

    pub(crate) fn driver(dev: impl BlockDriverOps + 'static) -> Self {
        Self::Driver(Box::new(dev))
    }
}

impl BlockDevice for Ext4Disk {
    fn read_blocks(&mut self, block_id: u64, buf: &mut [u8]) -> Ext4Result<usize> {
        #[cfg(feature = "profile")]
        let _profile = axsync::ProfileScope::new(
            axsync::ProfileEvent::BlockRead,
            self as *mut Self as usize,
        );
        match self {
            Self::Runtime(dev) => dev.read_block(block_id, buf),
            Self::Driver(dev) => dev.read_block(block_id, buf),
        }
        .map_err(|_| Ext4Error::new(EIO as _, None))?;
        Ok(buf.len())
    }

    fn write_blocks(&mut self, block_id: u64, buf: &[u8]) -> Ext4Result<usize> {
        #[cfg(feature = "profile")]
        let _profile = axsync::ProfileScope::new(
            axsync::ProfileEvent::BlockWrite,
            self as *mut Self as usize,
        );
        match self {
            Self::Runtime(dev) => dev.write_block(block_id, buf),
            Self::Driver(dev) => dev.write_block(block_id, buf),
        }
        .map_err(|_| Ext4Error::new(EIO as _, None))?;
        Ok(buf.len())
    }

    fn num_blocks(&self) -> Ext4Result<u64> {
        Ok(match self {
            Self::Runtime(dev) => dev.num_blocks(),
            Self::Driver(dev) => dev.num_blocks(),
        })
    }
}
