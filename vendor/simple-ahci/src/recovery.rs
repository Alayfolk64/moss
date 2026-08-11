//! Bounded SATA link resume, debounce, and COMRESET sequencing.

use volatile::VolatilePtr;

use crate::{
    Hal,
    completion::{DebounceResult, SataDebounce},
    hal::{WaitMode, delay_ms},
    mmio::{PortRegisters, PortRegistersVolatileFieldAccess},
};

const LINK_RESUME_RETRIES: usize = 5;
const LINK_RESUME_DELAY_MS: u64 = 200;
const LINK_DEBOUNCE_POLL_MS: u64 = 5;
const LINK_DEBOUNCE_STABLE_MS: u64 = 100;
const LINK_DEBOUNCE_TIMEOUT_MS: u64 = 2_000;
const RESET_SETTLE_MS: u64 = 150;
const READY_POLL_MS: u64 = 50;
const SCTL_DET_COMRESET: u32 = 1;
const SCTL_IPM_ACTIVE: u32 = 0x300;
const SCTL_SPD_MASK: u32 = 0x0f0;
const SCTL_RESUME_VERIFY_MASK: u32 = 0xf0f;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HardResetResult {
    Ready,
    Offline,
    Failed,
}

/// MMIO-only recovery primitive. Scheduling and command admission remain in
/// the port state machine; this type owns only the SATA timing sequence.
pub(crate) struct PortRecovery {
    port: VolatilePtr<'static, PortRegisters>,
}

impl PortRecovery {
    pub(crate) fn new(port: VolatilePtr<'static, PortRegisters>) -> Self {
        Self { port }
    }

    pub(crate) fn hard_reset<H: Hal>(
        &self,
        wait_mode: WaitMode,
        deadline_ms: u64,
    ) -> HardResetResult {
        // Keep the 1 ms COMRESET assertion free of a local IRQ window.
        let irqs_were_enabled = H::save_and_mask_local_irqs();
        let sctl =
            (self.port.SCTL().read() & SCTL_SPD_MASK) | SCTL_IPM_ACTIVE | SCTL_DET_COMRESET;
        self.port.SCTL().write(sctl);
        delay_ms::<H>(wait_mode, 1);
        H::restore_local_irqs(irqs_were_enabled);

        let Some(det) = self.resume_link::<H>(wait_mode, deadline_ms) else {
            return HardResetResult::Failed;
        };
        if det != 3 {
            return HardResetResult::Offline;
        }

        let remaining_ms = deadline_ms.saturating_sub(H::current_ms());
        if remaining_ms == 0 {
            return HardResetResult::Failed;
        }
        delay_ms::<H>(wait_mode, remaining_ms.min(RESET_SETTLE_MS));
        loop {
            let tfd = self.port.TFD().read();
            if !(tfd.STS_BSY() || tfd.STS_DRQ()) {
                return HardResetResult::Ready;
            }
            let remaining_ms = deadline_ms.saturating_sub(H::current_ms());
            if remaining_ms == 0 {
                return HardResetResult::Failed;
            }
            delay_ms::<H>(wait_mode, remaining_ms.min(READY_POLL_MS));
        }
    }

    fn resume_link<H: Hal>(&self, wait_mode: WaitMode, deadline_ms: u64) -> Option<u32> {
        for _ in 0..LINK_RESUME_RETRIES {
            let sctl = (self.port.SCTL().read() & SCTL_SPD_MASK) | SCTL_IPM_ACTIVE;
            self.port.SCTL().write(sctl);

            let remaining_ms = deadline_ms.saturating_sub(H::current_ms());
            if remaining_ms == 0 {
                return None;
            }
            // Avoid polling SStatus immediately after COMRESET release; some
            // PHYs fail negotiation when it is read continuously here.
            delay_ms::<H>(wait_mode, remaining_ms.min(LINK_RESUME_DELAY_MS));
            if self.port.SCTL().read() & SCTL_RESUME_VERIFY_MASK == SCTL_IPM_ACTIVE {
                let det = self.debounce_link::<H>(wait_mode, deadline_ms)?;
                self.port.SERR().write(self.port.SERR().read());
                return Some(det);
            }
        }
        None
    }

    fn debounce_link<H: Hal>(&self, wait_mode: WaitMode, deadline_ms: u64) -> Option<u32> {
        let started_ms = H::current_ms();
        let timeout_ms = deadline_ms
            .saturating_sub(started_ms)
            .min(LINK_DEBOUNCE_TIMEOUT_MS);
        if timeout_ms == 0 {
            return None;
        }

        let mut debounce = SataDebounce::new(
            started_ms,
            self.port.SSTS().read().DET().into(),
            LINK_DEBOUNCE_STABLE_MS,
            timeout_ms,
        );
        loop {
            let now_ms = H::current_ms();
            let remaining_ms = deadline_ms.saturating_sub(now_ms);
            if remaining_ms == 0 {
                return match debounce.observe(now_ms, self.port.SSTS().read().DET().into()) {
                    DebounceResult::Stable(det) => Some(det),
                    DebounceResult::Pending | DebounceResult::TimedOut => None,
                };
            }
            delay_ms::<H>(wait_mode, remaining_ms.min(LINK_DEBOUNCE_POLL_MS));
            match debounce.observe(H::current_ms(), self.port.SSTS().read().DET().into()) {
                DebounceResult::Pending => {}
                DebounceResult::Stable(det) => return Some(det),
                DebounceResult::TimedOut => return None,
            }
        }
    }
}
