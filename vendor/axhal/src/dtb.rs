//! DTB (Device Tree Blob) related functionality.
use core::ptr::NonNull;

use fdt_parser::{Fdt, Status};
use heapless::Vec;
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
    Some(cpu_hardware_ids()?.len())
}

const MAX_CPU_TOPOLOGY: usize = 256;

/// Returns enabled CPU IDs in dense logical order, with the boot CPU first.
///
/// The IDs come from each CPU node's `reg` property. This deliberately does
/// not infer IDs from node names or iteration order, since firmware-visible
/// CPU IDs are allowed to be sparse.
pub fn cpu_hardware_ids() -> Option<&'static [usize]> {
    static CACHED_CPU_IDS: LazyInit<Option<Vec<usize, MAX_CPU_TOPOLOGY>>> = LazyInit::new();

    if let Some(ids) = CACHED_CPU_IDS.get() {
        return ids.as_ref().map(Vec::as_slice);
    }

    fn discover() -> Option<Vec<usize, MAX_CPU_TOPOLOGY>> {
        let fdt = get_fdt()?;
        let mut ids = Vec::<usize, MAX_CPU_TOPOLOGY>::new();

        for node in fdt
            .find_nodes("/cpus/cpu")
            .filter(|node| node.status() != Some(Status::Disabled))
            .filter(|node| {
                node.find_property("device_type")
                    .is_none_or(|property| property.str() == "cpu")
            })
        {
            let id = node
                .reg()
                .and_then(|mut regs| regs.next())
                .map(|reg| reg.address as usize)
                .unwrap_or_else(|| panic!("enabled CPU node {} has no reg property", node.name()));
            assert!(!ids.contains(&id), "duplicate CPU hardware ID {id:#x} in DTB");
            ids.push(id).expect("too many CPU nodes in DTB");
        }

        if ids.is_empty() {
            return None;
        }

        // Firmware is free to emit CPU nodes in any order (QEMU LoongArch
        // currently emits them in descending order). Keep logical IDs stable
        // and dense before giving the boot CPU the distinguished first slot.
        ids.sort_unstable();
        let boot_id = fdt.boot_cpuid_phys() as usize;
        let boot_index = ids
            .iter()
            .position(|id| *id == boot_id)
            .unwrap_or_else(|| panic!("DTB boot CPU ID {boot_id:#x} is not enabled"));
        ids.swap(0, boot_index);
        Some(ids)
    }

    CACHED_CPU_IDS
        .call_once(discover)
        .or_else(|| CACHED_CPU_IDS.get())?
        .as_ref()
        .map(Vec::as_slice)
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
