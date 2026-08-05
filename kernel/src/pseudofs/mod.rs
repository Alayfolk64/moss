//! Basic virtual filesystem support

pub mod dev;
mod device;
mod dir;
mod file;
mod fs;
mod proc;
mod tmp;

use alloc::{format, string::String, sync::Arc};

use axerrno::LinuxResult;
use axfs::{FS_CONTEXT, File, FsContext};
use axfs_ng_vfs::{
    DirNodeOps, FileNodeOps, Filesystem, NodePermission, WeakDirEntry,
    path::{Path, PathBuf},
};
use axio::Write;
pub use tmp::MemoryFs;

pub use self::{device::*, dir::*, file::*, fs::*};

/// A callback that builds a `Arc<dyn DirNodeOps>` for a given
/// `WeakDirEntry`.
pub type DirMaker = Arc<dyn Fn(WeakDirEntry) -> Arc<dyn DirNodeOps> + Send + Sync>;

/// An enum containing either a directory ([`DirMaker`]) or a file (`Arc<dyn
/// FileNodeOps>`).
#[derive(Clone)]
pub enum NodeOpsMux {
    /// A directory node.
    Dir(DirMaker),
    /// A file node.
    File(Arc<dyn FileNodeOps>),
}

impl From<DirMaker> for NodeOpsMux {
    fn from(maker: DirMaker) -> Self {
        Self::Dir(maker)
    }
}

impl<T: FileNodeOps> From<Arc<T>> for NodeOpsMux {
    fn from(ops: Arc<T>) -> Self {
        Self::File(ops)
    }
}

const DIR_PERMISSION: NodePermission = NodePermission::from_bits_truncate(0o755);

fn mount_at(fs: &FsContext, path: &str, mount_fs: Filesystem) -> LinuxResult<()> {
    if fs.resolve(path).is_err() {
        fs.create_dir(path, DIR_PERMISSION)?;
    }
    fs.resolve(path)?.mount(&mount_fs)?;
    info!("Mounted {} at {}", mount_fs.name(), path);
    Ok(())
}

fn create_static_file(fs: &FsContext, path: &str, data: &[u8]) -> LinuxResult<()> {
    if fs.resolve(path).is_err() {
        let file = File::create(fs, path)?;
        let mut file = &file;
        file.write_all(data)?;
    }
    Ok(())
}

fn create_dir_all(fs: &FsContext, path: &str) -> LinuxResult<()> {
    let mut current = PathBuf::new();
    for comp in Path::new(path).components() {
        current.push(comp.as_str());
        if fs.resolve(&current).is_err() {
            fs.create_dir(&current, DIR_PERMISSION)?;
        }
    }
    Ok(())
}

fn cpu_range_string() -> String {
    match axhal::cpu_num() {
        0 | 1 => "0\n".into(),
        num => format!("0-{}\n", num - 1),
    }
}

/// Mount all filesystems
pub fn mount_all() -> LinuxResult<()> {
    info!("Initialize pseudofs...");

    let fs = FS_CONTEXT.lock();
    mount_at(&fs, "/dev", dev::new_devfs())?;
    mount_at(&fs, "/dev/shm", tmp::MemoryFs::new())?;
    mount_at(&fs, "/tmp", tmp::MemoryFs::new())?;
    mount_at(&fs, "/proc", proc::new_procfs())?;

    mount_at(&fs, "/sys", tmp::MemoryFs::new())?;
    create_dir_all(&fs, "/sys/class/graphics/fb0/device")?;
    let mut path = PathBuf::from("/sys/class/graphics/fb0/device");
    path.push("subsystem");
    fs.symlink("whatever", &path)?;

    create_dir_all(&fs, "/sys/block/loop0/queue")?;
    create_static_file(&fs, "/sys/block/loop0/queue/logical_block_size", b"512\n")?;
    create_static_file(&fs, "/sys/block/loop0/queue/dma_alignment", b"0\n")?;

    create_dir_all(&fs, "/sys/devices/system/cpu")?;
    for cpu_id in 0..axhal::cpu_num() {
        create_dir_all(&fs, &format!("/sys/devices/system/cpu/cpu{cpu_id}"))?;
    }
    let cpu_range = cpu_range_string();
    create_static_file(&fs, "/sys/devices/system/cpu/online", cpu_range.as_bytes())?;
    create_static_file(
        &fs,
        "/sys/devices/system/cpu/possible",
        cpu_range.as_bytes(),
    )?;
    create_static_file(&fs, "/sys/devices/system/cpu/present", cpu_range.as_bytes())?;
    create_static_file(&fs, "/sys/devices/system/cpu/offline", b"\n")?;
    drop(fs);

    #[cfg(feature = "dev-log")]
    dev::bind_dev_log().expect("Failed to bind /dev/log");

    Ok(())
}
