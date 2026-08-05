//! DTB (Device Tree Blob) related functionality.
use core::ptr::NonNull;

use fdt_parser::{Fdt, Status};
use lazyinit::LazyInit;

static BOOTARG: LazyInit<usize> = LazyInit::new();

/// Initializes the boot argument.
pub fn init(arg: usize) {
    if let Some(existing) = BOOTARG.get() {
        assert_eq!(*existing, arg, "boot DTB address changed after initialization");
    } else {
        BOOTARG.init_once(arg);
    }
}

/// Returns the boot argument.
/// This is typically the device tree blob address passed from the bootloader.
pub fn get_bootarg() -> usize {
    *BOOTARG
}

/// Get the cached FDT or initialize it if not already done.
pub fn get_fdt() -> Option<&'static Fdt<'static>> {
    static CACHED_FDT: LazyInit<Option<Fdt<'static>>> = LazyInit::new();

    // Return cached FDT if available
    if let Some(fdt) = CACHED_FDT.get() {
        return fdt.as_ref();
    }

    fn init_fdt() -> Option<Fdt<'static>> {
        let fdt_paddr = get_bootarg();
        let fdt_ptr = NonNull::new(crate::mem::phys_to_virt(fdt_paddr.into()).as_mut_ptr())?;
        Fdt::from_ptr(fdt_ptr).ok()
    }

    CACHED_FDT.init_once(init_fdt()).as_ref()
}

/// Returns the physical byte range occupied by the boot device tree.
pub fn physical_range() -> Option<(usize, usize)> {
    Some((get_bootarg(), get_fdt()?.total_size()))
}

/// Returns the number of enabled CPU nodes declared by the boot device tree.
pub fn cpu_count() -> Option<usize> {
    let count = get_fdt()?
        .find_nodes("/cpus/cpu")
        .filter(|node| node.status() != Some(Status::Disabled))
        .filter(|node| {
            node.find_property("device_type")
                .is_none_or(|property| property.str() == "cpu")
        })
        .count();

    (count != 0).then_some(count)
}

/// Get the bootargs chosen from the device tree.
pub fn get_chosen_bootargs() -> Option<&'static str> {
    static CACHED_BOOTARGS: LazyInit<Option<&'static str>> = LazyInit::new();

    // If bootargs are already cached, return them
    if let Some(bootargs) = CACHED_BOOTARGS.get() {
        return *bootargs;
    }

    fn init_bootargs() -> Option<&'static str> {
        let fdt = get_fdt()?;
        fdt.chosen()?.bootargs()
    }

    *CACHED_BOOTARGS.init_once(init_bootargs())
}
