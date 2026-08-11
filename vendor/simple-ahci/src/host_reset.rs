const AHCI_MAX_PORTS: usize = 32;
const PORT_CMD_CAP_MASK: u32 = 0x007c_0000;
const AHCI_VERSION_1_3: u32 = 0x0001_0300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PortMapSelection {
    pub(crate) value: u32,
    pub(crate) platform_override: bool,
    pub(crate) fabricated: bool,
}

/// Selects the operational PI mask using the same ordering as Linux libahci:
/// a non-zero platform value overrides HOST_PORTS_IMPL, masks wider than CAP.NP
/// are rejected, and only pre-1.3 controllers may fabricate PI from CAP.NP.
pub(crate) fn select_effective_port_map(
    cap: u32,
    version: u32,
    hardware_port_map: u32,
    platform_port_map: Option<u32>,
) -> PortMapSelection {
    let platform_port_map = platform_port_map.filter(|map| *map != 0);
    let platform_override = platform_port_map.is_some();
    let mut value = platform_port_map.unwrap_or(hardware_port_map);
    let port_count = usize::try_from(cap & 0x1f).unwrap() + 1;

    if value.count_ones() as usize > port_count {
        value = 0;
    }

    let fabricated = value == 0 && version < AHCI_VERSION_1_3;
    if fabricated {
        value = if port_count == AHCI_MAX_PORTS {
            u32::MAX
        } else {
            (1_u32 << port_count) - 1
        };
    }

    PortMapSelection {
        value,
        platform_override,
        fabricated,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterRestore {
    pub(crate) value: u32,
    pub(crate) write_needed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HostInitialConfig {
    pub(crate) cap: u32,
    pub(crate) version: u32,
    pub(crate) port_map: u32,
    pub(crate) port_cmd_caps: [u32; AHCI_MAX_PORTS],
}

impl HostInitialConfig {
    pub(crate) fn new(
        cap: u32,
        version: u32,
        port_map: u32,
        port_commands: [u32; AHCI_MAX_PORTS],
    ) -> Self {
        Self {
            cap,
            version,
            port_map,
            port_cmd_caps: port_commands.map(|command| command & PORT_CMD_CAP_MASK),
        }
    }

    pub(crate) fn port_map_restore(&self, observed_after_reset: u32) -> RegisterRestore {
        RegisterRestore {
            value: self.port_map,
            write_needed: observed_after_reset != self.port_map,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HostInitialConfig, select_effective_port_map};

    #[test]
    fn platform_port_map_overrides_unprogrammed_pi() {
        let selected = select_effective_port_map(0, 0x0001_0300, 0, Some(1));

        assert_eq!(selected.value, 1);
        assert!(selected.platform_override);
        assert!(!selected.fabricated);
    }

    #[test]
    fn rejects_a_port_map_wider_than_cap_np() {
        let selected = select_effective_port_map(0, 0x0001_0300, 0, Some(3));

        assert_eq!(selected.value, 0);
        assert!(!selected.fabricated);
    }

    #[test]
    fn fabricates_pi_from_np_only_before_ahci_1_3() {
        let selected = select_effective_port_map(0, 0x0001_0200, 0, None);

        assert_eq!(selected.value, 1);
        assert!(selected.fabricated);
    }

    #[test]
    fn leaves_unprogrammed_pi_empty_from_ahci_1_3_onward() {
        let selected = select_effective_port_map(0, 0x0001_0300, 0, None);

        assert_eq!(selected.value, 0);
        assert!(!selected.fabricated);
    }

    #[test]
    fn restores_firmware_port_map_when_host_reset_clears_pi() {
        let config = HostInitialConfig::new(0, 0x0001_0300, 1, [0; 32]);

        let restore = config.port_map_restore(0);

        assert_eq!(restore.value, 1);
        assert!(restore.write_needed);
    }

    #[test]
    fn preserves_only_platform_port_command_capabilities() {
        let mut commands = [0; 32];
        commands[0] = 0xffff_ffff;

        let config = HostInitialConfig::new(0, 0x0001_0300, 1, commands);

        assert_eq!(config.port_cmd_caps[0], 0x007c_0000);
        assert!(config.port_cmd_caps[1..].iter().all(|value| *value == 0));
    }
}
