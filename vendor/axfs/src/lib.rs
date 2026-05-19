#![cfg_attr(all(not(test), not(doc)), no_std)]
#![feature(doc_cfg)]
#![allow(clippy::new_ret_no_self)]

extern crate alloc;

#[macro_use]
extern crate log;

use axdriver::{AxBlockDevice, AxDeviceContainer, prelude::*};

#[cfg(feature = "fat")]
mod disk;
mod fs;

mod highlevel;
pub use highlevel::*;

#[cfg(feature = "ext4")]
pub use fs::ext4::Inode as Ext4Inode;

pub fn new_filesystem_from_block_device(dev: AxBlockDevice) -> axfs_ng_vfs::VfsResult<axfs_ng_vfs::Filesystem> {
    fs::new_default(dev)
}

pub fn init_filesystems(mut block_devs: AxDeviceContainer<AxBlockDevice>) {
    info!("Initialize filesystem subsystem...");

    let dev = block_devs.take_one().expect("No block device found!");
    info!("  use block device 0: {:?}", dev.device_name());

    let fs = fs::new_default(dev).expect("Failed to initialize filesystem");
    info!("  filesystem type: {:?}", fs.name());

    let mp = axfs_ng_vfs::Mountpoint::new_root(&fs);
    ROOT_FS_CONTEXT.call_once(|| FsContext::new(mp.root_location()));
}
