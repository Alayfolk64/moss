//! Controller-wide AHCI mode, firmware configuration capture, and reset restore.

use log::{error, info, warn};
use volatile::VolatilePtr;

use crate::{
    Hal,
    hal::{WaitMode, delay_ms},
    host_reset::{HostInitialConfig, PortMapSelection, select_effective_port_map},
    mmio::{
        AhciMmio, AhciMmioVolatileFieldAccess, CAP, GenericHostControlVolatileFieldAccess,
        PortRegisters, PortRegistersVolatileFieldAccess, PxCMD,
    },
};

const AHCI_ENABLE_RETRIES: usize = 5;
const AHCI_ENABLE_RETRY_MS: u64 = 10;

pub(crate) struct RestoredController {
    pub(crate) port_map: u32,
    port_map_was_cleared: bool,
}

/// Firmware-programmed host state that must survive HOST_RESET.
pub(crate) struct SavedControllerConfig {
    mmio: VolatilePtr<'static, AhciMmio>,
    initial: HostInitialConfig,
    selected_ports: PortMapSelection,
    hardware_port_map: u32,
    platform_port_map: Option<u32>,
}

impl SavedControllerConfig {
    pub(crate) fn capture<H: Hal>(
        mmio: VolatilePtr<'static, AhciMmio>,
        platform_port_map: Option<u32>,
    ) -> Option<Self> {
        if !enable_ahci::<H>(mmio) {
            error!("AHCI mode enable failed before HBA reset");
            return None;
        }

        let host = mmio.host();
        let cap = host.cap().read().into_bits();
        let version = host.vs().read().into_bits();
        let hardware_port_map = host.pi().read();
        let selected_ports =
            select_effective_port_map(cap, version, hardware_port_map, platform_port_map);
        if let Some(platform_ports) = platform_port_map.filter(|map| *map != 0) {
            if selected_ports.value != platform_ports {
                warn!(
                    "AHCI platform PI {platform_ports:#x} exceeds CAP.NP; effective PI is {:#x}",
                    selected_ports.value
                );
            }
        }

        let mut port_commands = [0_u32; 32];
        for (index, command) in port_commands.iter_mut().enumerate() {
            if selected_ports.value & (1_u32 << index) != 0 {
                let port = unsafe {
                    mmio.ports()
                        .map(|ports| ports.cast::<PortRegisters>().add(index))
                };
                *command = port.CMD().read().into_bits();
            }
        }

        Some(Self {
            mmio,
            initial: HostInitialConfig::new(cap, version, selected_ports.value, port_commands),
            selected_ports,
            hardware_port_map,
            platform_port_map,
        })
    }

    /// Re-enter AHCI mode and restore firmware state after HOST_RESET.
    pub(crate) fn restore_after_reset<H: Hal>(&self) -> Option<RestoredController> {
        if !enable_ahci::<H>(self.mmio) {
            error!("AHCI mode enable failed after HBA reset");
            return None;
        }

        let host = self.mmio.host();
        host.cap().write(CAP::from_bits(self.initial.cap));
        let restore = self.initial.port_map_restore(host.pi().read());
        host.pi().write(restore.value);
        let port_map = host.pi().read();
        for (index, command) in self.initial.port_cmd_caps.iter().enumerate() {
            if port_map & (1_u32 << index) != 0 {
                let port = unsafe {
                    self.mmio
                        .ports()
                        .map(|ports| ports.cast::<PortRegisters>().add(index))
                };
                port.CMD().write(PxCMD::from_bits(*command));
            }
        }

        Some(RestoredController {
            port_map,
            port_map_was_cleared: restore.write_needed,
        })
    }

    pub(crate) fn log_ready(&self, slot_count: usize, restored: &RestoredController) {
        info!(
            "AHCI host ready: version={:#x}, cap={:#x}, slots={slot_count}, \
             hardware_ports={:#x}, platform_ports={:?}, ports={:#x}, \
             platform_override={}, fabricated_ports={}, reset_cleared_ports={}",
            self.initial.version,
            self.initial.cap,
            self.hardware_port_map,
            self.platform_port_map,
            restored.port_map,
            self.selected_ports.platform_override,
            self.selected_ports.fabricated,
            restored.port_map_was_cleared
        );
    }
}

fn enable_ahci<H: Hal>(mmio: VolatilePtr<'static, AhciMmio>) -> bool {
    let host = mmio.host();
    for _ in 0..AHCI_ENABLE_RETRIES {
        let ghc = host.ghc().read();
        if ghc.AE() {
            return true;
        }
        host.ghc().write(ghc.with_AE(true).with_IE(false));
        if host.ghc().read().AE() {
            return true;
        }
        delay_ms::<H>(WaitMode::Poll, AHCI_ENABLE_RETRY_MS);
    }
    false
}
