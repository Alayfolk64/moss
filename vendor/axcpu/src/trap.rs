//! Trap handling.

use memory_addr::VirtAddr;

pub use crate::TrapFrame;
pub use linkme::distributed_slice as def_trap_handler;
pub use linkme::distributed_slice as register_trap_handler;
pub use page_table_entry::MappingFlags as PageFaultFlags;

/// A slice of IRQ handler functions.
#[def_trap_handler]
pub static IRQ: [fn(usize) -> bool];

/// A slice of page fault handler functions.
#[def_trap_handler]
pub static PAGE_FAULT: [fn(VirtAddr, PageFaultFlags) -> bool];

/// Guest profiling callbacks invoked with the interrupted CPU context.
#[cfg(feature = "profile")]
#[def_trap_handler]
pub static PROFILE_SAMPLE: [fn(&TrapFrame)];

/// Invokes the registered guest profiling callback.
#[cfg(feature = "profile")]
pub(crate) fn profile_sample(tf: &TrapFrame) {
    let mut iter = PROFILE_SAMPLE.iter();
    if let Some(func) = iter.next() {
        if iter.next().is_some() {
            warn!("Multiple guest profiling callbacks are not supported");
        }
        func(tf);
    }
}

#[allow(unused_macros)]
macro_rules! handle_trap {
    ($trap:ident, $($args:tt)*) => {{
        let mut iter = $crate::trap::$trap.iter();
        if let Some(func) = iter.next() {
            if iter.next().is_some() {
                warn!("Multiple handlers for trap {} are not currently supported", stringify!($trap));
            }
            func($($args)*)
        } else {
            warn!("No registered handler for trap {}", stringify!($trap));
            false
        }
    }}
}
