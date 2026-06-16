use core::{
    mem::{offset_of, size_of},
    time::Duration,
};

use super::{InodeRef, InodeType};
use crate::{SystemHal, ffi::*, util::get_block_size};

/// Filesystem node metadata.
#[derive(Clone, Debug, Default)]
pub struct FileAttr {
    /// ID of device containing file
    pub device: u64,
    /// Inode number
    pub ino: u32,
    /// Number of hard links
    pub nlink: u64,
    /// Permission mode
    pub mode: u32,
    /// Type of file
    pub node_type: InodeType,
    /// User ID of owner
    pub uid: u32,
    /// Group ID of owner
    pub gid: u32,
    /// Total size in bytes
    pub size: u64,
    /// Block size for filesystem I/O
    pub block_size: u64,
    /// Number of 512B blocks allocated
    pub blocks: u64,

    /// Time of last access
    pub atime: Duration,
    /// Time of last modification
    pub mtime: Duration,
    /// Time of last status change
    pub ctime: Duration,
}

fn encode_time(dur: &Duration) -> (u32, u32) {
    let sec = dur.as_secs();
    let nsec = dur.subsec_nanos();
    let time = u32::to_le(sec as u32);
    let extra = u32::to_le((nsec << 2) | (sec >> 32) as u32);
    (time, extra)
}

fn encode_time_without_extra(dur: &Duration) -> u32 {
    u32::to_le(dur.as_secs().min(i32::MAX as u64) as u32)
}

fn decode_time(time: u32, extra: u32) -> Duration {
    let sec = u32::from_le(time);
    let extra = u32::from_le(extra);
    let epoch = extra & 3;
    let nsec = extra >> 2;

    Duration::new(sec as u64 + ((epoch as u64) << 32), nsec)
}

#[derive(Clone, Copy)]
enum InodeTimeField {
    Access,
    Modification,
    Change,
}

impl InodeTimeField {
    fn extra_offset(self) -> usize {
        match self {
            Self::Access => offset_of!(ext4_inode, atime_extra),
            Self::Modification => offset_of!(ext4_inode, mtime_extra),
            Self::Change => offset_of!(ext4_inode, ctime_extra),
        }
    }

    fn base(self, inode: &ext4_inode) -> u32 {
        match self {
            Self::Access => inode.access_time,
            Self::Modification => inode.modification_time,
            Self::Change => inode.change_inode_time,
        }
    }

    fn extra(self, inode: &ext4_inode) -> u32 {
        match self {
            Self::Access => inode.atime_extra,
            Self::Modification => inode.mtime_extra,
            Self::Change => inode.ctime_extra,
        }
    }

    fn set(self, inode: &mut ext4_inode, time: u32, extra: Option<u32>) {
        match self {
            Self::Access => {
                inode.access_time = time;
                if let Some(extra) = extra {
                    inode.atime_extra = extra;
                }
            }
            Self::Modification => {
                inode.modification_time = time;
                if let Some(extra) = extra {
                    inode.mtime_extra = extra;
                }
            }
            Self::Change => {
                inode.change_inode_time = time;
                if let Some(extra) = extra {
                    inode.ctime_extra = extra;
                }
            }
        }
    }
}

impl<Hal: SystemHal> InodeRef<Hal> {
    fn inode_field_fits(&self, offset: usize, size: usize) -> bool {
        let inode_size = u16::from_le(self.superblock().inode_size) as usize;
        if inode_size <= EXT4_GOOD_OLD_INODE_SIZE as usize {
            return false;
        }

        let extra_isize = unsafe {
            ext4_inode_get_extra_isize(self.superblock() as *const _ as _, self.inner.inode)
                as usize
        };
        let Some(field_end) = offset.checked_add(size) else {
            return false;
        };
        field_end <= EXT4_GOOD_OLD_INODE_SIZE as usize + extra_isize && field_end <= inode_size
    }

    fn time_extra_fits(&self, field: InodeTimeField) -> bool {
        self.inode_field_fits(field.extra_offset(), size_of::<u32>())
    }

    fn set_time(&mut self, field: InodeTimeField, dur: &Duration) {
        let (time, extra) = if self.time_extra_fits(field) {
            let (time, extra) = encode_time(dur);
            (time, Some(extra))
        } else {
            (encode_time_without_extra(dur), None)
        };
        field.set(self.raw_inode_mut(), time, extra);
        self.mark_dirty();
    }

    fn get_time(&self, field: InodeTimeField) -> Duration {
        let inode = self.raw_inode();
        let extra = if self.time_extra_fits(field) {
            field.extra(inode)
        } else {
            0
        };
        decode_time(field.base(inode), extra)
    }

    pub fn inode_type(&self) -> InodeType {
        ((self.mode() >> 12) as u8).into()
    }

    pub fn is_dir(&self) -> bool {
        self.inode_type() == InodeType::Directory
    }

    pub fn size(&self) -> u64 {
        unsafe { ext4_inode_get_size(self.superblock() as *const _ as _, self.inner.inode) }
    }

    pub fn mode(&self) -> u32 {
        unsafe { ext4_inode_get_mode(self.superblock() as *const _ as _, self.inner.inode) }
    }
    pub fn set_mode(&mut self, mode: u32) {
        unsafe {
            ext4_inode_set_mode(self.superblock_mut(), self.inner.inode, mode);
            self.mark_dirty();
        }
    }

    pub fn nlink(&self) -> u16 {
        u16::from_le(self.raw_inode().links_count)
    }

    pub fn uid(&self) -> u16 {
        u16::from_le(self.raw_inode().uid)
    }
    pub fn gid(&self) -> u16 {
        u16::from_le(self.raw_inode().gid)
    }

    pub fn set_owner(&mut self, uid: u16, gid: u16) {
        let inode = self.raw_inode_mut();
        inode.uid = u16::to_le(uid);
        inode.gid = u16::to_le(gid);
        self.mark_dirty();
    }

    pub fn set_atime(&mut self, dur: &Duration) {
        self.set_time(InodeTimeField::Access, dur);
    }
    pub fn set_mtime(&mut self, dur: &Duration) {
        self.set_time(InodeTimeField::Modification, dur);
    }
    pub fn set_ctime(&mut self, dur: &Duration) {
        self.set_time(InodeTimeField::Change, dur);
    }

    pub fn update_atime(&mut self) {
        if let Some(dur) = Hal::now() {
            self.set_atime(&dur);
        }
    }
    pub fn update_mtime(&mut self) {
        if let Some(dur) = Hal::now() {
            self.set_mtime(&dur);
        }
    }
    pub fn update_ctime(&mut self) {
        if let Some(dur) = Hal::now() {
            self.set_ctime(&dur);
        }
    }

    pub fn get_attr(&self, attr: &mut FileAttr) {
        attr.device = 0;
        attr.ino = u32::from_le(self.inner.index);
        attr.nlink = self.nlink() as _;
        attr.mode = self.mode();
        attr.node_type = self.inode_type();
        attr.uid = self.uid() as _;
        attr.gid = self.gid() as _;
        attr.size = self.size();
        attr.block_size = get_block_size(self.superblock()) as _;
        attr.blocks = unsafe {
            ext4_inode_get_blocks_count(self.superblock() as *const _ as _, self.inner.inode)
        };

        attr.atime = self.get_time(InodeTimeField::Access);
        attr.mtime = self.get_time(InodeTimeField::Modification);
        attr.ctime = self.get_time(InodeTimeField::Change);
    }
}
