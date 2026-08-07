mod block_io;
mod fs;
mod inode;
mod util;

use axdriver::AxBlockDevice;
pub use fs::*;
pub use inode::*;
use lwext4_rust::{BlockDevice, Ext4Error, Ext4Result, ffi::EIO};

use self::block_io::SharedBlockDevice;

pub fn enable_async_block_io() {
    block_io::enable_async_block_io();
}

#[derive(Clone)]
pub(crate) struct Ext4Disk(SharedBlockDevice);

impl Ext4Disk {
    pub(crate) fn runtime(dev: AxBlockDevice) -> Self {
        Self(SharedBlockDevice::runtime(dev))
    }

    pub(crate) fn driver(dev: impl axdriver::prelude::BlockDriverOps + 'static) -> Self {
        Self(SharedBlockDevice::driver(dev))
    }

    pub(crate) fn shared(&self) -> SharedBlockDevice {
        self.0.clone()
    }
}

impl BlockDevice for Ext4Disk {
    fn read_blocks(&mut self, block_id: u64, buf: &mut [u8]) -> Ext4Result<usize> {
        self.0
            .read_blocks(block_id, buf)
            .map_err(|_| Ext4Error::new(EIO as _, None))?;
        Ok(buf.len())
    }

    fn write_blocks(&mut self, block_id: u64, buf: &[u8]) -> Ext4Result<usize> {
        self.0
            .write_blocks(block_id, buf)
            .map_err(|_| Ext4Error::new(EIO as _, None))?;
        Ok(buf.len())
    }

    fn num_blocks(&self) -> Ext4Result<u64> {
        Ok(self.0.num_blocks())
    }
}
