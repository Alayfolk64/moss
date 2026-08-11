//! Pure AHCI port state transitions shared by probe and unit tests.

const PORT_CMD_ST: u32 = 1 << 0;
const PORT_CMD_FRE: u32 = 1 << 4;
const PORT_CMD_FR: u32 = 1 << 14;
const PORT_CMD_CR: u32 = 1 << 15;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortDeinitAction {
    ClearStart,
    WaitCommandEngine,
    ClearFisReceive,
    WaitFisReceive,
    Complete,
}

pub(crate) const fn next_port_deinit_action(command: u32) -> PortDeinitAction {
    if command & PORT_CMD_ST != 0 {
        PortDeinitAction::ClearStart
    } else if command & PORT_CMD_CR != 0 {
        PortDeinitAction::WaitCommandEngine
    } else if command & PORT_CMD_FRE != 0 {
        PortDeinitAction::ClearFisReceive
    } else if command & PORT_CMD_FR != 0 {
        PortDeinitAction::WaitFisReceive
    } else {
        PortDeinitAction::Complete
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PORT_CMD_CR, PORT_CMD_FR, PORT_CMD_FRE, PORT_CMD_ST, PortDeinitAction,
        next_port_deinit_action,
    };

    #[test]
    fn clears_start_before_waiting_for_command_engine() {
        assert_eq!(
            next_port_deinit_action(PORT_CMD_ST | PORT_CMD_CR | PORT_CMD_FRE | PORT_CMD_FR),
            PortDeinitAction::ClearStart
        );
        assert_eq!(
            next_port_deinit_action(PORT_CMD_CR | PORT_CMD_FRE | PORT_CMD_FR),
            PortDeinitAction::WaitCommandEngine
        );
    }

    #[test]
    fn leaves_fis_receive_enabled_until_command_engine_stops() {
        assert_eq!(
            next_port_deinit_action(PORT_CMD_FRE | PORT_CMD_FR),
            PortDeinitAction::ClearFisReceive
        );
    }

    #[test]
    fn waits_for_fis_receive_after_clearing_fre() {
        assert_eq!(
            next_port_deinit_action(PORT_CMD_FR),
            PortDeinitAction::WaitFisReceive
        );
    }

    #[test]
    fn reports_complete_only_when_both_engines_are_idle() {
        assert_eq!(next_port_deinit_action(0), PortDeinitAction::Complete);
    }
}
