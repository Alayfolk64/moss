//! Power management.

/// Power management interface.
#[def_plat_interface]
pub trait PowerIf {
    /// Bootstraps the given CPU core with the given initial stack (in physical
    /// address).
    ///
    /// Where `cpu_id` is the hardware CPU ID reported by firmware.
    #[cfg(feature = "smp")]
    fn cpu_boot(cpu_id: usize, stack_top_paddr: usize);

    /// Reboot the whole system. Platforms without a reset mechanism fall back
    /// to their safe power-off implementation.
    fn system_reset() -> ! {
        Self::system_off()
    }

    /// Starts the hardware watchdog with the requested timeout.
    ///
    /// Returns `false` when the platform has no usable watchdog.
    fn watchdog_start(_timeout_secs: u32) -> bool {
        false
    }

    /// Reloads the hardware watchdog countdown.
    fn watchdog_ping() {}

    /// Stops the hardware watchdog.
    fn watchdog_stop() {}

    /// Shutdown the whole system.
    fn system_off() -> !;
}
