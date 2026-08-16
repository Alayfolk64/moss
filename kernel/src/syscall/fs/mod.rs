mod aio;
mod at_path;
mod ctl;
mod event;
mod fanotify;
mod fd_ops;
mod inotify;
mod io;
mod memfd;
mod mount;
mod pidfd;
mod pipe;
mod signalfd;
mod stat;
mod xattr;

use axsync::Mutex;

/// Serializes namespace operations whose decisions depend on a pathname or
/// link-count snapshot. `axfs::FS_CONTEXT` is scope-local, so its lock cannot
/// provide cross-process exclusion.
pub(super) static NAMESPACE_TRANSACTION: Mutex<()> = Mutex::new(());

pub use self::{
    aio::*, ctl::*, event::*, fanotify::*, fd_ops::*, inotify::*, io::*, memfd::*, mount::*,
    pidfd::*, pipe::*, signalfd::*, stat::*, xattr::*,
};
