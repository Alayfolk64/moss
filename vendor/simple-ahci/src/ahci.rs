use alloc::{alloc::alloc_zeroed, boxed::Box, sync::Arc, vec::Vec};
use core::{
    alloc::Layout,
    marker::PhantomData,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use log::{debug, error, info, warn};
use spin::Mutex;
use volatile::VolatilePtr;

use crate::{
    Hal,
    ata::{
        ATA_CMD_FLUSH, ATA_CMD_FLUSH_EXT, ATA_CMD_ID_ATA, ATA_CMD_READ, ATA_CMD_READ_EXT,
        ATA_CMD_WRITE, ATA_CMD_WRITE_EXT, ATA_ID_FW_REV, ATA_ID_FW_REV_LEN, ATA_ID_PROD,
        ATA_ID_PROD_LEN, ATA_ID_SERNO, ATA_ID_SERNO_LEN, ATA_ID_WORDS, SATA_FIS_TYPE_REGISTER_H2D,
        ata_id_has_lba48, ata_id_n_sectors, ata_id_to_string,
    },
    completion::{
        AdmissionTicket, CommandToken, CompletionQueue, DebounceResult, NonNcqAdmission,
        RecoveryAction, SataDebounce, command_completion_failed, completed_commands,
        port_interrupt_failed, recovery_action, transfer_in_range,
    },
    hal::{WaitMode, WaitModeState, delay_ms, wait_for_event_or_spin, wait_until_timeout},
    host_reset::{HostInitialConfig, select_effective_port_map},
    mmio::{
        AhciMmio, AhciMmioVolatileFieldAccess, CAP, GenericHostControlVolatileFieldAccess, ICC,
        PortRegisters, PortRegistersVolatileFieldAccess, PxCMD, PxI,
    },
    port_state::{PortDeinitAction, next_port_deinit_action},
    types::{
        AHCI_MAX_BYTES_PER_CMD, AHCI_MAX_BYTES_PER_SG, AHCI_MAX_SG, ahci_cmd_hdr, ahci_cmd_list,
        ahci_cmd_tbl, ahci_cmd_tblVolatileFieldAccess, ahci_rx_fis, ahci_sg, sata_fis_h2d,
    },
};

const COMMAND_TIMEOUT_MS: u64 = 1_000;
const COMMAND_COMPLETION_POLL_MS: u64 = 10;
const AHCI_ENABLE_RETRIES: usize = 5;
const AHCI_ENABLE_RETRY_MS: u64 = 10;
const ENGINE_STOP_TIMEOUT_MS: u64 = 500;
const FIS_STOP_TIMEOUT_MS: u64 = 1_000;
const LINK_RESUME_RETRIES: usize = 5;
const LINK_RESUME_DELAY_MS: u64 = 200;
const LINK_DEBOUNCE_POLL_MS: u64 = 5;
const LINK_DEBOUNCE_STABLE_MS: u64 = 100;
const LINK_DEBOUNCE_TIMEOUT_MS: u64 = 2_000;
const RESET_SETTLE_MS: u64 = 150;
const READY_POLL_MS: u64 = 50;
const OFFLINE_RETRY_MS: u64 = 60_000;
const RESET_TIMEOUT_MS: u64 = 5_000;
const SCTL_DET_COMRESET: u32 = 1;
const SCTL_IPM_ACTIVE: u32 = 0x300;
const SCTL_SPD_MASK: u32 = 0x0f0;
const SCTL_RESUME_VERIFY_MASK: u32 = 0xf0f;
fn alloc<T: Sized>(align: usize) -> VolatilePtr<'static, T> {
    unsafe {
        VolatilePtr::new(NonNull::new_unchecked(
            alloc_zeroed(Layout::from_size_align(size_of::<T>(), align).unwrap()).cast(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HardResetResult {
    Ready,
    Offline,
    Failed,
}

/// DMA memory owned by one AHCI port. Allocation and descriptor construction
/// are kept separate from MMIO state changes, matching Linux's port_start
/// split between memory setup and engine activation.
struct AhciPortDma {
    cmd_list: VolatilePtr<'static, ahci_cmd_list>,
    #[allow(dead_code)]
    fis: VolatilePtr<'static, ahci_rx_fis>,
    cmd_tables: Vec<VolatilePtr<'static, ahci_cmd_tbl>>,
}

impl AhciPortDma {
    fn allocate<H: Hal>(slot_count: usize) -> Self {
        let cmd_list = alloc::<ahci_cmd_list>(1024);
        let fis = alloc::<ahci_rx_fis>(256);
        let mut cmd_tables = Vec::with_capacity(slot_count);

        for slot in 0..slot_count {
            let table = alloc::<ahci_cmd_tbl>(128);
            let table_addr = H::virt_to_phys(table.as_raw_ptr().addr().get());
            unsafe { cmd_list.map(|list| list.cast::<ahci_cmd_hdr>().add(slot)) }.write(
                ahci_cmd_hdr {
                    tbl_addr_lo: table_addr as u32,
                    tbl_addr_hi: (table_addr >> 32) as u32,
                    ..Default::default()
                },
            );
            cmd_tables.push(table);
        }

        H::flush_dcache();
        Self {
            cmd_list,
            fis,
            cmd_tables,
        }
    }

    fn program<H: Hal>(&self, port: VolatilePtr<'static, PortRegisters>) {
        let cmd_list_addr = H::virt_to_phys(self.cmd_list.as_raw_ptr().addr().get());
        port.CLB().write(cmd_list_addr as u32);
        port.CLBU().write((cmd_list_addr >> 32) as u32);

        let fis_addr = H::virt_to_phys(self.fis.as_raw_ptr().addr().get());
        port.FB().write(fis_addr as u32);
        port.FBU().write((fis_addr >> 32) as u32);
    }
}

struct AhciPort<H> {
    host: VolatilePtr<'static, AhciMmio>,
    port: VolatilePtr<'static, PortRegisters>,
    index: u8,
    dma: AhciPortDma,
    completion: CompletionQueue,
    admission: NonNcqAdmission,
    issue_lock: Mutex<()>,
    recovering: AtomicBool,
    recovery_needed: AtomicBool,
    controller_failed: AtomicBool,
    recovery_retry_after_ms: AtomicU64,
    interrupt_mode: AtomicBool,
    polled_completion_reported: AtomicBool,
    wait_mode: WaitModeState,
    _h: PhantomData<H>,
}

unsafe impl<H: Hal> Send for AhciPort<H> {}
unsafe impl<H: Hal> Sync for AhciPort<H> {}

impl<H: Hal> AhciPort<H> {
    /// Reinitialize synchronization state after the port reaches its final
    /// heap allocation but before recovery, IRQ publication or command issue.
    ///
    /// The pinned LoongArch compiler used by this board has corrupted atomic
    /// fields while relocating large aggregates. Keeping the port behind a
    /// Box prevents all later moves; explicit stable-address stores also make
    /// the initial state independent of the construction move itself.
    fn reinitialize_unpublished(&mut self) {
        self.completion.reinitialize_unpublished();
        self.admission.reinitialize_unpublished();
        unsafe { core::ptr::write(&mut self.issue_lock, Mutex::new(())) };
        self.recovering.store(false, Ordering::Relaxed);
        self.recovery_needed.store(true, Ordering::Relaxed);
        self.controller_failed.store(false, Ordering::Relaxed);
        self.recovery_retry_after_ms.store(0, Ordering::Relaxed);
        self.interrupt_mode.store(false, Ordering::Relaxed);
        self.polled_completion_reported
            .store(false, Ordering::Relaxed);
        self.wait_mode.reinitialize_unpublished();
    }

    /// Stop an initialized or firmware-owned port in the order required by
    /// AHCI 1.3.1 and Linux ahci_deinit_port(): stop the command engine first,
    /// then stop FIS reception. PxCMD.FRE must not be cleared while PxCMD.CR is
    /// still set.
    fn deinit_port(port: VolatilePtr<'static, PortRegisters>) -> bool {
        loop {
            match next_port_deinit_action(port.CMD().read().into_bits()) {
                PortDeinitAction::ClearStart => {
                    port.CMD().update(|cmd| cmd.with_ST(false));
                }
                PortDeinitAction::WaitCommandEngine => {
                    if !wait_until_timeout::<H>(|| !port.CMD().read().CR(), ENGINE_STOP_TIMEOUT_MS)
                    {
                        return false;
                    }
                }
                PortDeinitAction::ClearFisReceive => {
                    port.CMD().update(|cmd| cmd.with_FRE(false));
                }
                PortDeinitAction::WaitFisReceive => {
                    if !wait_until_timeout::<H>(|| !port.CMD().read().FR(), FIS_STOP_TIMEOUT_MS) {
                        return false;
                    }
                }
                PortDeinitAction::Complete => return true,
            }
        }
    }

    fn clear_pending(
        host: VolatilePtr<'static, AhciMmio>,
        port: VolatilePtr<'static, PortRegisters>,
        index: u8,
    ) {
        let serr = port.SERR().read();
        port.SERR().write(serr);
        let status = port.IS().read();
        if status.into_bits() != 0 {
            port.IS().write(status);
        }
        host.host().is().write(1_u32 << index);
    }

    fn power_up(port: VolatilePtr<'static, PortRegisters>) {
        port.CMD()
            .update(|cmd| cmd.with_ICC(ICC::Active).with_POD(true).with_SUD(true));
    }

    fn start_fis_receive(port: VolatilePtr<'static, PortRegisters>) {
        port.CMD().update(|cmd| cmd.with_FRE(true));
        let _ = port.CMD().read();
    }

    fn start_command_engine(&self) {
        self.port.CMD().update(|cmd| cmd.with_ST(true));
        let _ = self.port.CMD().read();
    }

    fn try_new(
        host: VolatilePtr<'static, AhciMmio>,
        index: u8,
        slot_count: usize,
    ) -> Option<Box<Self>> {
        let port = unsafe {
            host.ports()
                .map(|ports| ports.cast::<PortRegisters>().add(index as usize))
        };

        port.IE().write(PxI::new());
        if !Self::deinit_port(port) {
            warn!(
                "AHCI port {index} failed to stop before initialization: CMD={:?}",
                port.CMD().read()
            );
            return None;
        }

        // Linux ahci_init_controller() does not inspect PxTFD or issue CLO
        // while the receive-FIS area is still absent.  Establish the DMA/FIS
        // state first; the subsequent EH hard reset handles device-busy state.
        Self::clear_pending(host, port, index);
        let dma = AhciPortDma::allocate::<H>(slot_count);
        dma.program::<H>(port);
        Self::power_up(port);
        Self::start_fis_receive(port);

        let mut candidate = Box::new(Self {
            host,
            port,
            index,
            dma,
            completion: CompletionQueue::new(slot_count),
            admission: NonNcqAdmission::new(),
            issue_lock: Mutex::new(()),
            recovering: AtomicBool::new(false),
            // Probe follows the same EH hard-reset path as runtime errors, so
            // slow PHY negotiation and slow device ready transitions receive
            // Linux-compatible debounce and reset deadlines.
            recovery_needed: AtomicBool::new(true),
            controller_failed: AtomicBool::new(false),
            recovery_retry_after_ms: AtomicU64::new(0),
            interrupt_mode: AtomicBool::new(false),
            polled_completion_reported: AtomicBool::new(false),
            wait_mode: WaitModeState::new_polling(),
            _h: PhantomData,
        });
        candidate.reinitialize_unpublished();
        debug_assert_eq!(candidate.current_recovery_action(), RecoveryAction::Recover);
        if !candidate.recover_if_needed(WaitMode::Poll) {
            warn!(
                "AHCI port {index} did not become ready during initial recovery: action={:?} \
                 recovering={} recovery_needed={} controller_failed={} retry_after_ms={} \
                 SSTS={:?} TFD={:?} CMD={:?}",
                candidate.current_recovery_action(),
                candidate.recovering.load(Ordering::Relaxed),
                candidate.recovery_needed.load(Ordering::Relaxed),
                candidate.controller_failed.load(Ordering::Relaxed),
                candidate.recovery_retry_after_ms.load(Ordering::Relaxed),
                candidate.port.SSTS().read(),
                candidate.port.TFD().read(),
                candidate.port.CMD().read(),
            );
            return None;
        }
        info!(
            "AHCI port {index} ready: slots={slot_count}, SSTS={:?}, TFD={:?}",
            candidate.port.SSTS().read(),
            candidate.port.TFD().read()
        );
        Some(candidate)
    }

    fn enable_interrupts(&self) {
        self.port.IS().write(self.port.IS().read());
        self.host.host().is().write(1_u32 << self.index);
        self.port.IE().write(PxI::default_enable().with_DP(true));
        self.host.host().ghc().update(|ghc| ghc.with_IE(true));
        self.interrupt_mode.store(true, Ordering::Release);
    }

    fn enable_event_waits(&self) {
        self.wait_mode.enable_event_waits();
    }

    fn wait_mode(&self) -> WaitMode {
        self.wait_mode.current()
    }

    fn current_recovery_action(&self) -> RecoveryAction {
        recovery_action(
            self.controller_failed.load(Ordering::Acquire),
            self.recovering.load(Ordering::Acquire),
            self.recovery_needed.load(Ordering::Acquire),
            H::current_ms(),
            self.recovery_retry_after_ms.load(Ordering::Acquire),
        )
    }

    fn release_admission(&self, ticket: AdmissionTicket) {
        self.admission.release(ticket);
        H::notify_event();
    }

    fn reserve_slot(&self) -> Option<(CommandToken, AdmissionTicket)> {
        let admission = self.admission.reserve();
        loop {
            // Ordinary non-NCQ contention is not an I/O timeout. Wait in FIFO
            // order; the hardware watchdog remains the bound for a genuinely
            // wedged owner. Rechecking once per command timeout also covers a
            // lost event notification without abandoning this ticket.
            while !self.admission.is_turn(admission) {
                let _ =
                    wait_for_event_or_spin::<H, _>(self.wait_mode(), COMMAND_TIMEOUT_MS, || {
                        self.admission.is_turn(admission)
                    });
            }

            match self.current_recovery_action() {
                RecoveryAction::Fail | RecoveryAction::Defer => {
                    self.release_admission(admission);
                    return None;
                }
                RecoveryAction::Recover => {
                    if !self.recover_if_needed(self.wait_mode()) {
                        self.release_admission(admission);
                        return None;
                    }
                    continue;
                }
                RecoveryAction::Wait => {
                    if !wait_for_event_or_spin::<H, _>(self.wait_mode(), COMMAND_TIMEOUT_MS, || {
                        self.current_recovery_action() != RecoveryAction::Wait
                    }) {
                        continue;
                    }
                    continue;
                }
                RecoveryAction::Admit => {}
            }

            let Some(selected) = self.completion.try_reserve() else {
                error!("AHCI FIFO admission reached a non-free command slot");
                self.release_admission(admission);
                return None;
            };
            return Some((selected, admission));
        }
    }

    fn prepare_command(
        &self,
        token: CommandToken,
        cfis: sata_fis_h2d,
        buffer_address: usize,
        buffer_len: usize,
        is_write: bool,
    ) -> bool {
        if buffer_len > AHCI_MAX_BYTES_PER_CMD {
            error!("AHCI transfer exceeds per-command limit");
            return false;
        }
        let table = &self.dma.cmd_tables[token.slot];
        table.hdr().write(cfis);

        let sg_count = if buffer_len == 0 {
            0
        } else {
            let count = (buffer_len - 1) / AHCI_MAX_BYTES_PER_SG + 1;
            if count > AHCI_MAX_SG {
                return false;
            }
            let mut remaining = buffer_len;
            for index in 0..count {
                let offset = index * AHCI_MAX_BYTES_PER_SG;
                let len = remaining.min(AHCI_MAX_BYTES_PER_SG);
                let addr = H::virt_to_phys(buffer_address + offset);
                unsafe { table.sgs().map(|sg| sg.cast::<ahci_sg>().add(index)) }.write(ahci_sg {
                    addr_lo: addr as u32,
                    addr_hi: (addr >> 32) as u32,
                    flags_size: (len - 1) as u32 & 0x3f_ffff,
                    ..Default::default()
                });
                remaining -= len;
            }
            count
        };

        let table_addr = H::virt_to_phys(table.as_raw_ptr().addr().get());
        let opts = (size_of::<sata_fis_h2d>() / 4) as u32
            | ((sg_count as u32) << 16)
            | ((is_write as u32) << 6);
        unsafe {
            self.dma
                .cmd_list
                .map(|list| list.cast::<ahci_cmd_hdr>().add(token.slot))
        }
        .write(ahci_cmd_hdr {
            opts,
            tbl_addr_lo: table_addr as u32,
            tbl_addr_hi: (table_addr >> 32) as u32,
            ..Default::default()
        });
        true
    }

    fn issue(&self, token: CommandToken) -> bool {
        loop {
            let guard = self.issue_lock.lock();
            if self.current_recovery_action() != RecoveryAction::Admit {
                drop(guard);
                if !self.recover_if_needed(self.wait_mode()) {
                    return false;
                }
                continue;
            }

            let bit = 1_u32 << token.slot;
            H::flush_dcache();
            // PxCI is write-one-to-set and hardware clears completed slots. A
            // read-modify-write can resurrect a slot that completed between
            // the read and write, so publish only the newly issued bit.
            self.port.CI().write(bit);
            H::flush_dcache();
            // Publish after PxCI: an interrupt on another CPU must never see a
            // slot that has not reached hardware. If hardware completed before
            // publication, the immediate PxCI check below closes that window.
            self.completion.mark_issued(token);
            let recovery_needed = self.recovery_needed.load(Ordering::Acquire);
            if self.interrupt_mode.load(Ordering::Acquire)
                && (recovery_needed || self.port.CI().read() & bit == 0)
            {
                let status_bits = self.port.IS().read().into_bits();
                let failed = command_completion_failed(
                    recovery_needed,
                    status_bits,
                    self.port.TFD().read().STS_ERR(),
                );
                if failed {
                    self.recovery_needed.store(true, Ordering::Release);
                }
                self.completion.publish_checked(bit, failed, || {
                    let failed = command_completion_failed(
                        self.recovery_needed.load(Ordering::Acquire),
                        self.port.IS().read().into_bits(),
                        self.port.TFD().read().STS_ERR(),
                    );
                    if failed {
                        self.recovery_needed.store(true, Ordering::Release);
                    }
                    failed
                });
                H::notify_event();
            }
            return true;
        }
    }

    fn finish_token(&self, token: CommandToken, admission: AdmissionTicket) -> bool {
        let completed = if self.interrupt_mode.load(Ordering::Acquire) {
            let started_ms = H::current_ms();
            loop {
                if self.poll_token_completion(token) {
                    break true;
                }
                let elapsed_ms = H::current_ms().saturating_sub(started_ms);
                if elapsed_ms >= COMMAND_TIMEOUT_MS {
                    break false;
                }
                let wait_ms = COMMAND_TIMEOUT_MS
                    .saturating_sub(elapsed_ms)
                    .min(COMMAND_COMPLETION_POLL_MS);
                let _ = wait_for_event_or_spin::<H, _>(self.wait_mode(), wait_ms, || {
                    self.poll_token_completion(token)
                });
            }
        } else {
            wait_until_timeout::<H>(
                || self.port.CI().read() & (1_u32 << token.slot) == 0,
                COMMAND_TIMEOUT_MS,
            )
        };

        if !completed {
            error!(
                "AHCI command timeout: port={} slot={} CI={:#x} IS={:?} TFD={:?}",
                self.index,
                token.slot,
                self.port.CI().read(),
                self.port.IS().read(),
                self.port.TFD().read()
            );
            self.recovery_needed.store(true, Ordering::Release);
            self.recover_if_needed(self.wait_mode());
        } else if !self.interrupt_mode.load(Ordering::Acquire) {
            let failed = self.port.TFD().read().STS_ERR();
            let status = self.port.IS().read();
            self.port.IS().write(status);
            self.host.host().is().write(1_u32 << self.index);
            if failed {
                self.recovery_needed.store(true, Ordering::Release);
            }
            self.completion.publish(1_u32 << token.slot, failed);
        }

        H::flush_dcache();
        let result = self.completion.take(token);
        if result == Some(false) {
            self.recover_if_needed(self.wait_mode());
        }
        self.release_admission(admission);
        result == Some(true)
    }

    /// Complete a command from PxCI when the controller's level interrupt did
    /// not reach the CPU. The completion queue's atomic claim makes this safe
    /// against an IRQ handler racing on another CPU.
    fn poll_token_completion(&self, token: CommandToken) -> bool {
        if self.completion.is_done(token) {
            return true;
        }

        let command_bit = 1_u32 << token.slot;
        if self.port.CI().read() & command_bit != 0 {
            return false;
        }

        let status = self.port.IS().read();
        let status_bits = status.into_bits();
        let failed = command_completion_failed(
            self.recovery_needed.load(Ordering::Acquire),
            status_bits,
            self.port.TFD().read().STS_ERR(),
        );
        if failed {
            self.recovery_needed.store(true, Ordering::Release);
        }
        let published = self.completion.publish_checked(command_bit, failed, || {
            let failed = command_completion_failed(
                self.recovery_needed.load(Ordering::Acquire),
                self.port.IS().read().into_bits(),
                self.port.TFD().read().STS_ERR(),
            );
            if failed {
                self.recovery_needed.store(true, Ordering::Release);
            }
            failed
        });
        if published != 0 {
            // AHCI is level-triggered. Clear the port source before the HBA
            // summary, matching the normal IRQ handler's acknowledgement
            // order. Any status that arrived after our snapshot remains set.
            self.port.IS().write(status);
            self.host.host().is().write(1_u32 << self.index);
            if !self.polled_completion_reported.swap(true, Ordering::AcqRel) {
                warn!(
                    "AHCI port {} recovered a missed completion IRQ by polling PxCI",
                    self.index
                );
            }
        }
        self.completion.is_done(token)
    }

    fn exec_cmd_buffer(
        &self,
        cfis: sata_fis_h2d,
        buffer_address: usize,
        buffer_len: usize,
        is_write: bool,
    ) -> bool {
        let Some((token, admission)) = self.reserve_slot() else {
            error!("AHCI command admission failed");
            return false;
        };
        if !self.prepare_command(token, cfis, buffer_address, buffer_len, is_write) {
            self.completion.cancel(token);
            self.release_admission(admission);
            return false;
        }
        if !self.issue(token) {
            self.completion.cancel(token);
            self.release_admission(admission);
            return false;
        }
        self.finish_token(token, admission)
    }

    fn exec_cmd(&self, cfis: sata_fis_h2d, buf: &mut [u8], is_write: bool) -> bool {
        self.exec_cmd_buffer(cfis, buf.as_mut_ptr() as usize, buf.len(), is_write)
    }

    fn handle_interrupt(&self) -> bool {
        let host_status = self.host.host().is().read();
        let port_bit = 1_u32 << self.index;
        if host_status & port_bit == 0 {
            return false;
        }

        let status = self.port.IS().read();
        let status_bits = status.into_bits();
        let failed = port_interrupt_failed(status_bits);
        if failed {
            // Publish the fatal state before acknowledging PxIS or sampling
            // the software-issued bitmap. A submitter may be between its PxCI
            // write and mark_issued(); recovery_needed is the hand-off that
            // prevents cleared interrupt status from becoming false success.
            self.recovery_needed.store(true, Ordering::Release);
        }
        self.port.IE().write(PxI::new());
        self.port.IS().write(PxI::from_bits(status_bits));

        let active = self.completion.issued();
        let finished = completed_commands(active, self.port.CI().read(), failed);
        self.completion.publish(finished, failed);

        // AHCI uses a level interrupt: acknowledge every PxIS first and HBA IS
        // last, matching libahci's ordering.
        self.host.host().is().write(host_status);
        if !failed {
            self.port.IE().write(PxI::default_enable().with_DP(true));
        }
        H::notify_event();
        true
    }

    fn debounce_link(&self, wait_mode: WaitMode, deadline_ms: u64) -> Option<u32> {
        let started_ms = H::current_ms();
        let timeout_ms = deadline_ms
            .saturating_sub(started_ms)
            .min(LINK_DEBOUNCE_TIMEOUT_MS);
        if timeout_ms == 0 {
            return None;
        }

        let initial_det = self.port.SSTS().read().DET();
        let mut debounce = SataDebounce::new(
            started_ms,
            initial_det.into(),
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

    fn resume_link(&self, wait_mode: WaitMode, deadline_ms: u64) -> Option<u32> {
        for _ in 0..LINK_RESUME_RETRIES {
            let sctl = (self.port.SCTL().read() & SCTL_SPD_MASK) | SCTL_IPM_ACTIVE;
            self.port.SCTL().write(sctl);

            let remaining_ms = deadline_ms.saturating_sub(H::current_ms());
            if remaining_ms == 0 {
                return None;
            }
            // Match sata_link_resume(): some PHYs fail negotiation when SStatus
            // is read repeatedly immediately after COMRESET is released.
            delay_ms::<H>(wait_mode, remaining_ms.min(LINK_RESUME_DELAY_MS));
            if self.port.SCTL().read() & SCTL_RESUME_VERIFY_MASK == SCTL_IPM_ACTIVE {
                let det = self.debounce_link(wait_mode, deadline_ms)?;
                self.port.SERR().write(self.port.SERR().read());
                return Some(det);
            }
        }
        None
    }

    fn hard_reset(&self, wait_mode: WaitMode, deadline_ms: u64) -> HardResetResult {
        // Mask before writing PxSCTL so the A/B experiment has no interrupt
        // window between the COMRESET write/readback log and the delay.
        let irqs_were_enabled = H::save_and_mask_local_irqs();
        let sctl = (self.port.SCTL().read() & SCTL_SPD_MASK) | SCTL_IPM_ACTIVE | SCTL_DET_COMRESET;
        self.port.SCTL().write(sctl);

        // SATA requires COMRESET to remain asserted for at least 1 ms.
        delay_ms::<H>(wait_mode, 1);
        H::restore_local_irqs(irqs_were_enabled);

        let Some(det) = self.resume_link(wait_mode, deadline_ms) else {
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

    fn recover_if_needed(&self, wait_mode: WaitMode) -> bool {
        loop {
            match self.current_recovery_action() {
                RecoveryAction::Admit => return true,
                RecoveryAction::Fail => return false,
                RecoveryAction::Defer => return false,
                RecoveryAction::Wait => {
                    if !wait_for_event_or_spin::<H, _>(wait_mode, COMMAND_TIMEOUT_MS, || {
                        self.current_recovery_action() != RecoveryAction::Wait
                    }) {
                        return false;
                    }
                }
                RecoveryAction::Recover => {
                    if self
                        .recovering
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        break;
                    }
                }
            }
        }

        let issue_guard = self.issue_lock.lock();
        if !self.recovery_needed.swap(false, Ordering::AcqRel) {
            self.recovering.store(false, Ordering::Release);
            H::notify_event();
            return !self.controller_failed.load(Ordering::Acquire);
        }

        self.port.IE().write(PxI::new());
        let active = self.completion.issued();
        self.completion.publish(active, true);

        // Linux libahci freezes the port, stops the command engine, and then
        // performs an EH reset. This compact driver has no libata EH thread,
        // so recovery is serialized here and uses the same COMRESET primitive.
        self.port.CMD().update(|cmd| cmd.with_ST(false));
        // `recovering` now excludes new command issue, so the spin lock has
        // finished its job. Release it before any wait that may park the task.
        drop(issue_guard);
        if !wait_for_event_or_spin::<H, _>(wait_mode, ENGINE_STOP_TIMEOUT_MS, || {
            !self.port.CMD().read().CR()
        }) {
            // Linux treats failure to stop PxCMD.CR as a controller failure;
            // further resets can race an engine still fetching descriptors.
            self.controller_failed.store(true, Ordering::Release);
            error!(
                "AHCI port {} command engine did not stop: CMD={:?}",
                self.index,
                self.port.CMD().read()
            );
        } else {
            // Keep PxCMD.FRE enabled while resetting so the HBA can receive
            // the reset D2H FIS, as ahci_do_hardreset() does.
            self.port.SERR().write(self.port.SERR().read());
            self.port.IS().write(self.port.IS().read());
            self.host.host().is().write(1_u32 << self.index);

            // Keep recovery bounded. This driver has no independent libata EH
            // worker, so copying Linux's multi-attempt 10/10/35/5 second
            // schedule here blocks the caller synchronously for too long.
            // One five-second COMRESET is enough to classify this I/O attempt;
            // a failed/offline port remains eligible for the deferred retry
            // below instead of monopolising the current task.
            let reset_started_ms = H::current_ms();
            let deadline_ms = reset_started_ms.saturating_add(RESET_TIMEOUT_MS);
            debug!(
                "AHCI port {} recovery starts: now_ms={}, timeout_ms={}",
                self.index, reset_started_ms, RESET_TIMEOUT_MS
            );
            let result = self.hard_reset(wait_mode, deadline_ms);
            debug!(
                "AHCI port {} recovery finished {:?}: now_ms={}, SSTS={:?}, TFD={:?}",
                self.index,
                result,
                H::current_ms(),
                self.port.SSTS().read(),
                self.port.TFD().read()
            );

            if result == HardResetResult::Ready {
                self.port.SERR().write(self.port.SERR().read());
                self.port.IS().write(self.port.IS().read());
                self.host.host().is().write(1_u32 << self.index);
                self.start_command_engine();
                self.recovery_retry_after_ms.store(0, Ordering::Release);
                if self.interrupt_mode.load(Ordering::Acquire) {
                    self.port.IE().write(PxI::default_enable().with_DP(true));
                }
            } else {
                // Link/drive failures are retryable. Linux thaws a failed port
                // and permits a later PHY/probe event instead of poisoning it
                // permanently; without an EH thread, retry on the next I/O
                // after the same one-minute probe interval.
                self.port.IE().write(PxI::new());
                self.recovery_needed.store(true, Ordering::Release);
                self.recovery_retry_after_ms.store(
                    H::current_ms().saturating_add(OFFLINE_RETRY_MS),
                    Ordering::Release,
                );
                warn!(
                    "AHCI port {} recovery {:?}; retry deferred for {} ms: SSTS={:?} TFD={:?}",
                    self.index,
                    result,
                    OFFLINE_RETRY_MS,
                    self.port.SSTS().read(),
                    self.port.TFD().read()
                );
            }
        }

        self.recovering.store(false, Ordering::Release);
        H::notify_event();
        !self.controller_failed.load(Ordering::Acquire)
            && !self.recovery_needed.load(Ordering::Acquire)
    }
}

struct AhciInner<H> {
    _mmio: VolatilePtr<'static, AhciMmio>,
    port: Box<AhciPort<H>>,
}

unsafe impl<H: Hal> Send for AhciInner<H> {}
unsafe impl<H: Hal> Sync for AhciInner<H> {}

/// A stable interrupt endpoint that remains valid when the block device moves.
pub struct AhciIrqHandle<H> {
    inner: Arc<AhciInner<H>>,
}

impl<H> Clone for AhciIrqHandle<H> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<H: Hal> AhciIrqHandle<H> {
    pub fn handle_interrupt(&self) -> bool {
        self.inner.port.handle_interrupt()
    }
}

/// AHCI controller prepared with device interrupts still disabled.
pub struct PreparedAhci<H> {
    inner: Arc<AhciInner<H>>,
}

impl<H: Hal> PreparedAhci<H> {
    pub fn irq_handle(&self) -> AhciIrqHandle<H> {
        AhciIrqHandle {
            inner: self.inner.clone(),
        }
    }

    pub fn enable_interrupts(&self) {
        self.inner.port.enable_interrupts();
    }

    pub fn identify(self) -> Option<AhciDriver<H>> {
        let mut id = [0_u16; ATA_ID_WORDS];
        if !self.inner.port.exec_cmd(
            sata_fis_h2d {
                fis_type: SATA_FIS_TYPE_REGISTER_H2D,
                pm_port_c: 0x80,
                command: ATA_CMD_ID_ATA,
                ..Default::default()
            },
            unsafe {
                core::slice::from_raw_parts_mut(id.as_mut_ptr().cast::<u8>(), size_of_val(&id))
            },
            false,
        ) {
            return None;
        }

        info!(
            "AHCI device: {} {} {}",
            ata_id_to_string(&id, ATA_ID_PROD, ATA_ID_PROD_LEN),
            ata_id_to_string(&id, ATA_ID_SERNO, ATA_ID_SERNO_LEN),
            ata_id_to_string(&id, ATA_ID_FW_REV, ATA_ID_FW_REV_LEN)
        );
        Some(AhciDriver {
            inner: self.inner,
            block_size: 512,
            max_lba: ata_id_n_sectors(&id),
            is_lba48: ata_id_has_lba48(&id),
        })
        .filter(|driver| driver.max_lba != 0)
    }
}

pub struct AhciDriver<H> {
    inner: Arc<AhciInner<H>>,
    block_size: usize,
    max_lba: u64,
    is_lba48: bool,
}

unsafe impl<H: Hal> Send for AhciDriver<H> {}
unsafe impl<H: Hal> Sync for AhciDriver<H> {}

impl<H: Hal> AhciDriver<H> {
    /// Prepares an AHCI controller without enabling device interrupts or
    /// issuing IDENTIFY. This lets the platform publish and register the IRQ
    /// endpoint before the first command can sleep.
    pub unsafe fn prepare(base: usize) -> Option<PreparedAhci<H>> {
        unsafe { Self::prepare_with_port_map(base, None) }
    }

    /// Prepares a platform AHCI controller, optionally overriding an
    /// unprogrammed HOST_PORTS_IMPL register with the firmware PI mask.
    pub unsafe fn prepare_with_port_map(
        base: usize,
        platform_port_map: Option<u32>,
    ) -> Option<PreparedAhci<H>> {
        let mmio = unsafe { VolatilePtr::new(NonNull::new(base as *mut _).unwrap()) };
        let host = mmio.host();

        // Match Linux ahci_enable_ahci(): configuration registers are valid
        // only in AHCI mode, and some controllers require repeated AE writes.
        let enable_ahci = || {
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
        };
        if !enable_ahci() {
            error!("AHCI mode enable failed before HBA reset");
            return None;
        }

        // Linux ahci_save_initial_config() runs before HOST_RESET because
        // platform/firmware configuration such as PI and PxCMD capability bits
        // may be cleared by reset. LS2K1000 clears PI in practice.
        let saved_cap = host.cap().read().into_bits();
        let saved_version = host.vs().read().into_bits();
        let hardware_port_map = host.pi().read();
        let selected_port_map = select_effective_port_map(
            saved_cap,
            saved_version,
            hardware_port_map,
            platform_port_map,
        );
        let saved_port_map = selected_port_map.value;
        if let Some(platform_port_map) = platform_port_map.filter(|map| *map != 0) {
            if saved_port_map != platform_port_map {
                warn!(
                    "AHCI platform PI {platform_port_map:#x} exceeds CAP.NP; effective PI is \
                     {saved_port_map:#x}"
                );
            }
        }
        let mut saved_port_commands = [0_u32; 32];
        for (index, command) in saved_port_commands.iter_mut().enumerate() {
            if saved_port_map & (1_u32 << index) != 0 {
                let port = unsafe {
                    mmio.ports()
                        .map(|ports| ports.cast::<PortRegisters>().add(index))
                };
                *command = port.CMD().read().into_bits();
            }
        }
        let initial_config = HostInitialConfig::new(
            saved_cap,
            saved_version,
            saved_port_map,
            saved_port_commands,
        );

        host.ghc().update(|ghc| ghc.with_IE(false).with_HR(true));
        if !wait_until_timeout::<H>(|| !host.ghc().read().HR(), 1_000) {
            error!("AHCI HBA reset timeout");
            return None;
        }
        if !enable_ahci() {
            error!("AHCI mode enable failed after HBA reset");
            return None;
        }

        // Match Linux ahci_restore_initial_config(). Restore PI before touching
        // port registers and read it back to flush the MMIO write.
        host.cap().write(CAP::from_bits(initial_config.cap));
        let port_map_restore = initial_config.port_map_restore(host.pi().read());
        host.pi().write(port_map_restore.value);
        let port_map = host.pi().read();
        for (index, command) in initial_config.port_cmd_caps.iter().enumerate() {
            if port_map & (1_u32 << index) != 0 {
                let port = unsafe {
                    mmio.ports()
                        .map(|ports| ports.cast::<PortRegisters>().add(index))
                };
                port.CMD().write(PxCMD::from_bits(*command));
            }
        }

        let cap = host.cap().read();
        let slot_count = usize::from(cap.NCS()) + 1;
        let mut selected = None;
        for index in 0..=cap.NP() {
            if port_map & (1_u32 << index) != 0 {
                if let Some(port) = AhciPort::<H>::try_new(mmio, index, slot_count) {
                    selected = Some(port);
                    break;
                }
            }
        }
        let port = selected?;
        // Linux prints the host summary only after reset and controller/port
        // initialization. Keep this path numeric and compact: verbose CAP
        // formatting is diagnostic work, not part of an MMIO state transition.
        info!(
            "AHCI host ready: version={:#x}, cap={:#x}, slots={slot_count}, \
             hardware_ports={hardware_port_map:#x}, platform_ports={platform_port_map:?}, \
             ports={port_map:#x}, platform_override={}, fabricated_ports={}, \
             reset_cleared_ports={}",
            initial_config.version,
            initial_config.cap,
            selected_port_map.platform_override,
            selected_port_map.fabricated,
            port_map_restore.write_needed
        );
        Some(PreparedAhci {
            inner: Arc::new(AhciInner { _mmio: mmio, port }),
        })
    }

    /// Compatibility constructor for environments without a registered IRQ.
    pub unsafe fn try_new(base: usize) -> Option<Self> {
        let driver = unsafe { Self::prepare(base) }?.identify()?;
        driver.enable_runtime_waits();
        Some(driver)
    }

    /// Completes platform probing and allows later command paths to use the
    /// HAL event wait hook when their current task is blockable.
    pub fn enable_runtime_waits(&self) {
        self.inner.port.enable_event_waits();
    }

    pub fn capacity(&self) -> u64 {
        self.max_lba
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn read(&self, block_id: u64, buf: &mut [u8]) -> bool {
        if !transfer_in_range(block_id, buf.len(), self.block_size, self.max_lba) {
            return false;
        }
        unsafe { self.rw_common(block_id, buf.as_mut_ptr(), buf.len(), false) }
    }

    pub fn write(&self, block_id: u64, buf: &[u8]) -> bool {
        if !transfer_in_range(block_id, buf.len(), self.block_size, self.max_lba) {
            return false;
        }
        unsafe { self.rw_common(block_id, buf.as_ptr() as *mut u8, buf.len(), true) }
    }

    pub fn flush(&self) -> bool {
        let command = if self.is_lba48 {
            ATA_CMD_FLUSH_EXT
        } else {
            ATA_CMD_FLUSH
        };
        self.inner.port.exec_cmd(
            sata_fis_h2d {
                fis_type: SATA_FIS_TYPE_REGISTER_H2D,
                pm_port_c: 0x80,
                command,
                ..Default::default()
            },
            &mut [],
            false,
        )
    }

    /// # Safety
    ///
    /// `buffer` must remain valid for `buffer_len` bytes until this synchronous
    /// operation returns. It must be writable when `is_write` is false.
    unsafe fn rw_common(
        &self,
        block_id: u64,
        buffer: *mut u8,
        buffer_len: usize,
        is_write: bool,
    ) -> bool {
        let mut start = block_id;
        let mut remaining = buffer_len;
        let mut offset = 0;
        while remaining != 0 {
            let sectors = remaining.div_ceil(self.block_size);
            let max_sectors = if self.is_lba48 { 65_536 } else { 256 };
            let count = sectors
                .min(max_sectors)
                .min(AHCI_MAX_BYTES_PER_CMD / self.block_size);
            let bytes = (count * self.block_size).min(remaining);
            let mut fis = sata_fis_h2d {
                fis_type: SATA_FIS_TYPE_REGISTER_H2D,
                pm_port_c: 0x80,
                ..Default::default()
            };
            if self.is_lba48 {
                fis.command = if is_write {
                    ATA_CMD_WRITE_EXT
                } else {
                    ATA_CMD_READ_EXT
                };
                fis.lba_low = start as u8;
                fis.lba_mid = (start >> 8) as u8;
                fis.lba_high = (start >> 16) as u8;
                fis.lba_low_exp = (start >> 24) as u8;
                fis.lba_mid_exp = (start >> 32) as u8;
                fis.lba_high_exp = (start >> 40) as u8;
                fis.device = 0x40;
                fis.sector_count = count as u8;
                fis.sector_count_exp = (count >> 8) as u8;
            } else {
                fis.command = if is_write {
                    ATA_CMD_WRITE
                } else {
                    ATA_CMD_READ
                };
                fis.lba_low = start as u8;
                fis.lba_mid = (start >> 8) as u8;
                fis.lba_high = (start >> 16) as u8;
                fis.device = 0x40 | ((start >> 24) as u8 & 0x0f);
                fis.sector_count = count as u8;
            }

            let address = unsafe { buffer.add(offset) } as usize;
            if address % 4 == 0 {
                if !self
                    .inner
                    .port
                    .exec_cmd_buffer(fis, address, bytes, is_write)
                {
                    return false;
                }
            } else {
                let mut bounce = alloc::vec![0_u8; bytes];
                if is_write {
                    bounce.copy_from_slice(unsafe {
                        core::slice::from_raw_parts(buffer.add(offset), bytes)
                    });
                }
                if !self.inner.port.exec_cmd(fis, &mut bounce, is_write) {
                    return false;
                }
                if !is_write {
                    unsafe {
                        core::ptr::copy_nonoverlapping(bounce.as_ptr(), buffer.add(offset), bytes)
                    };
                }
            }
            start += count as u64;
            remaining -= bytes;
            offset += bytes;
        }
        true
    }
}
