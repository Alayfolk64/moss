use core::{ptr::NonNull, time::Duration};

use log::{debug, info, trace, warn};
use volatile::VolatilePtr;

use crate::{
    cmd::{Command, DataXfer},
    idmac::{self, Descriptor, DmaConfig},
    interrupt::{IDMAC_COMPLETE, IDMAC_ERROR, InterruptWait},
    regs::{
        ClkDiv, ClkEna, IntMask, RIntSts, RegisterBlock, RegisterBlockVolatileFieldAccess, TmOut,
    },
    utils::{Cid, CsdV2},
};

/// An SD/MMC block transfer error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SdMmcError {
    /// A block range or DMA buffer does not meet the controller requirements.
    InvalidParameter,
    /// The controller rejected or failed a command after all retry attempts.
    CommandFailed,
    /// The controller could not be returned to an idle state after an error.
    RecoveryFailed,
    /// The card did not finish programming data before the flush deadline.
    FlushTimeout,
}

fn wait_until<F>(mut f: F)
where
    F: FnMut() -> bool,
{
    // TODO: yield?
    while !f() {
        core::hint::spin_loop();
    }
}

/// Enables or disables the controller's global interrupt output.
///
/// This is used by the platform's top-half handler to deassert a level-triggered
/// interrupt before the PLIC claim is completed.
///
/// # Safety
///
/// `base` must point to the SD/MMC controller register block owned by this driver.
pub unsafe fn set_interrupt_enabled(base: usize, enabled: bool) {
    let regs = unsafe { VolatilePtr::new(NonNull::new_unchecked(base as *mut RegisterBlock)) };
    regs.ctrl().update(|r| r.with_int_enable(enabled));
}

/// SD/MMC driver.
pub struct SdMmc {
    regs: VolatilePtr<'static, RegisterBlock>,
    num_blocks: u64,
    interrupt: Option<&'static dyn InterruptWait>,
    dma: Option<DmaConfig>,
    dma_reported: bool,
}

impl SdMmc {
    const FIFO: usize = 0x200;
    const COMMAND_TIMEOUT_MS: u64 = 2_000;
    const FLUSH_TIMEOUT_MS: u64 = 10_000;
    const RESET_TIMEOUT_MS: u64 = 100;
    const BLOCK_ATTEMPTS: usize = 3;
    const IDMAC_FIFO_THRESHOLD: u32 = 0x300f_0010;

    /// Creates a new `SdMmc` instance from the given base address.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `base` is a valid pointer to the SD/MMC controller's
    /// register block and that no other code is concurrently accessing the same hardware.
    pub unsafe fn new(base: usize) -> Self {
        let regs = unsafe { VolatilePtr::new(NonNull::new_unchecked(base as *mut _)) };

        let mut this = Self {
            regs,
            num_blocks: 0,
            interrupt: None,
            dma: None,
            dma_reported: false,
        };
        this.init();
        this
    }

    /// Creates an SD/MMC instance using controller completion interrupts.
    ///
    /// # Safety
    ///
    /// The caller must uphold the same MMIO ownership requirements as [`Self::new`]
    /// and ensure that `interrupt` is connected to this controller's IRQ.
    pub unsafe fn new_with_interrupt(base: usize, interrupt: &'static dyn InterruptWait) -> Self {
        let regs = unsafe { VolatilePtr::new(NonNull::new_unchecked(base as *mut _)) };
        let mut this = Self {
            regs,
            num_blocks: 0,
            interrupt: Some(interrupt),
            dma: None,
            dma_reported: false,
        };
        this.init();
        this
    }

    /// Creates an interrupt-driven SD/MMC instance with the internal DMA engine.
    ///
    /// # Safety
    ///
    /// The caller must uphold the MMIO and interrupt requirements of
    /// [`Self::new_with_interrupt`] and the memory requirements of
    /// [`DmaConfig::new`].
    pub unsafe fn new_with_interrupt_and_dma(
        base: usize,
        interrupt: &'static dyn InterruptWait,
        dma: DmaConfig,
    ) -> Self {
        assert!(dma.descriptors.is_multiple_of(16));
        assert!(dma.buffer.is_multiple_of(4096));
        assert!(dma.max_transfer_len() >= Self::BLOCK_SIZE);
        let regs = unsafe { VolatilePtr::new(NonNull::new_unchecked(base as *mut _)) };
        let mut this = Self {
            regs,
            num_blocks: 0,
            interrupt: Some(interrupt),
            dma: Some(dma),
            dma_reported: false,
        };
        this.init();
        info!(
            "SD/MMC IDMAC enabled with {} KiB bounce buffer",
            dma.max_transfer_len() / 1024
        );
        this
    }

    fn can_send_cmd(&self) -> bool {
        !self.regs.cmd().read().start_cmd()
    }

    fn can_send_data(&self) -> bool {
        !self.regs.status().read().data_busy()
    }

    fn has_response(&self) -> bool {
        self.regs.rintsts().read().command_done()
    }

    fn fifo_cnt(&self) -> usize {
        self.regs.status().read().fifo_count() as usize
    }

    fn fifo_word_bytes(&self) -> usize {
        match self.regs.hcon().read().h_data_width() {
            0 => 2,
            1 => 4,
            2 => 8,
            width => panic!("unsupported SD/MMC host data width {width}"),
        }
    }

    fn read_fifo_word(&self, buf: &mut [u8]) {
        let fifo = unsafe { self.regs.as_raw_ptr().byte_add(Self::FIFO) };
        match buf.len() {
            2 => buf.copy_from_slice(&unsafe { fifo.cast::<u16>().read_volatile() }.to_le_bytes()),
            4 => buf.copy_from_slice(&unsafe { fifo.cast::<u32>().read_volatile() }.to_le_bytes()),
            8 => buf.copy_from_slice(&unsafe { fifo.cast::<u64>().read_volatile() }.to_le_bytes()),
            len => panic!("unsupported SD/MMC FIFO read size {len}"),
        }
    }

    fn write_fifo_word(&self, buf: &[u8]) {
        let fifo = unsafe { self.regs.as_raw_ptr().byte_add(Self::FIFO) };
        match buf.len() {
            2 => unsafe {
                fifo.cast::<u16>()
                    .write_volatile(u16::from_le_bytes(buf.try_into().unwrap()))
            },
            4 => unsafe {
                fifo.cast::<u32>()
                    .write_volatile(u32::from_le_bytes(buf.try_into().unwrap()))
            },
            8 => unsafe {
                fifo.cast::<u64>()
                    .write_volatile(u64::from_le_bytes(buf.try_into().unwrap()))
            },
            len => panic!("unsupported SD/MMC FIFO write size {len}"),
        }
    }

    fn set_transaction_size(&self, blk_size: u16, byte_cnt: u32) {
        self.regs.blksiz().update(|r| r.with_block_size(blk_size));
        self.regs.bytcnt().write(byte_cnt);
    }

    fn configure_timeouts(&self) {
        // Do not depend on timeout values inherited from the previous payload.
        // The maximum data timeout also gives an interrupt-driven PIO request
        // enough time to service the FIFO under scheduler load.
        self.regs.tmout().write(
            TmOut::new()
                .with_data_timeout(0x00ff_ffff)
                .with_response_timeout(0xff),
        );
    }

    fn configure_interrupt_mask(&self) {
        self.regs.intmask().write(
            IntMask::new()
                .with_ebe(true)
                .with_sbe(true)
                .with_hle(true)
                .with_frun(true)
                .with_hto(true)
                .with_drto(true)
                .with_rto(true)
                .with_dcrc(true)
                .with_rcrc(true)
                .with_rxdr(true)
                .with_txdr(true)
                .with_dto(true)
                .with_cmd(true)
                .with_re(true),
        );
    }

    fn wait_until_bounded<F>(&self, timeout_ms: u64, mut condition: F) -> bool
    where
        F: FnMut() -> bool,
    {
        if let Some(interrupt) = self.interrupt {
            let deadline = interrupt.current_ms().saturating_add(timeout_ms);
            while !condition() {
                if interrupt.current_ms() >= deadline {
                    return false;
                }
                core::hint::spin_loop();
            }
            true
        } else {
            // Polling-only platforms have no clock hook in this small driver.
            // Keep recovery bounded even if the controller never clears reset.
            for _ in 0..1_000_000 {
                if condition() {
                    return true;
                }
                core::hint::spin_loop();
            }
            false
        }
    }

    fn recover_data_path(&self) -> bool {
        self.regs.ctrl().update(|r| {
            r.with_int_enable(false)
                .with_dma_enable(false)
                .with_use_internal_dmac(false)
                .with_controller_reset(true)
                .with_fifo_reset(true)
                .with_dma_reset(true)
        });

        let reset_done = self.wait_until_bounded(Self::RESET_TIMEOUT_MS, || {
            let ctrl = self.regs.ctrl().read();
            !ctrl.controller_reset() && !ctrl.fifo_reset() && !ctrl.dma_reset()
        });

        let status = self.regs.rintsts().read();
        self.regs.rintsts().write(status);
        self.configure_timeouts();

        if let Some(interrupt) = self.interrupt {
            self.configure_interrupt_mask();
            interrupt.finish();
            interrupt.rearm();
        }

        reset_done
    }

    fn send_cmd(&self, command: Command<'_>) -> Option<[u32; 4]> {
        if self.interrupt.is_some() {
            self.send_cmd_interrupt(command)
        } else {
            self.send_cmd_polling(command)
        }
    }

    fn send_cmd_polling(&self, command: Command<'_>) -> Option<[u32; 4]> {
        trace!("send_cmd {command:#x?}");

        let (cmd, arg, xfer) = command.build();
        assert_eq!(cmd.data_expected(), xfer.is_some());

        trace!("send_cmd {cmd:?} {arg:#x?}");

        wait_until(|| self.can_send_cmd());
        if cmd.data_expected() {
            wait_until(|| self.can_send_data());
        }

        self.regs.cmdarg().write(arg);
        self.regs.cmd().write(cmd);

        wait_until(|| self.can_send_cmd());
        trace!("cmd {} sent", cmd.cmd_index());

        if cmd.response_expect() {
            wait_until(|| self.has_response());
            trace!("cmd {} received response", cmd.cmd_index());
        }

        if let Some(xfer) = xfer {
            let word_bytes = self.fifo_word_bytes();
            let fifo_locations = word_bytes.div_ceil(4);
            let mut offset = 0;
            match xfer {
                DataXfer::Read(buf) => {
                    wait_until(|| {
                        let rintsts = self.regs.rintsts().read();

                        if rintsts.receive_fifo_data_request() {
                            trace!("rxdr");
                            while self.fifo_cnt() >= fifo_locations
                                && offset + word_bytes <= buf.len()
                            {
                                self.read_fifo_word(&mut buf[offset..offset + word_bytes]);
                                offset += word_bytes;
                            }
                        }

                        rintsts.data_transfer_over() || rintsts.error()
                    });
                    trace!("received {offset} bytes");
                }
                DataXfer::Write(buf) => {
                    wait_until(|| {
                        let rintsts = self.regs.rintsts().read();

                        if rintsts.transmit_fifo_data_request() {
                            trace!("txdr");
                            // Hard coded FIFO depth
                            while self.fifo_cnt() < 120 && offset + word_bytes <= buf.len() {
                                self.write_fifo_word(&buf[offset..offset + word_bytes]);
                                offset += word_bytes;
                            }
                        }

                        rintsts.data_transfer_over() || rintsts.error()
                    });
                    trace!("sent {offset} bytes");
                }
            }
        }

        let resp = self.regs.resp().read();

        let rintsts = self.regs.rintsts().read();
        // clear interrupt status
        self.regs.rintsts().write(rintsts);

        if rintsts.error() {
            trace!("cmd {} error: {rintsts:?} resp: {resp:?}", cmd.cmd_index());
            return None;
        }
        Some(resp)
    }

    fn wait_until_interrupt<F>(&self, timeout_ms: u64, mut condition: F) -> bool
    where
        F: FnMut() -> bool,
    {
        let interrupt = self.interrupt.unwrap();
        let deadline = interrupt.current_ms().saturating_add(timeout_ms);
        while !condition() {
            if interrupt.current_ms() >= deadline {
                return false;
            }
            core::hint::spin_loop();
        }
        true
    }

    fn send_cmd_interrupt(&self, command: Command<'_>) -> Option<[u32; 4]> {
        trace!("send_cmd {command:#x?}");
        let (cmd, arg, mut xfer) = command.build();
        assert_eq!(cmd.data_expected(), xfer.is_some());

        if !self.wait_until_interrupt(1_000, || self.can_send_cmd())
            || (cmd.data_expected() && !self.wait_until_interrupt(1_000, || self.can_send_data()))
        {
            warn!(
                "SD/MMC controller stayed busy before CMD{}",
                cmd.cmd_index()
            );
            return None;
        }

        // Clock-update pseudo commands never produce Command Done interrupts.
        if cmd.update_clock_registers_only() {
            self.regs.cmdarg().write(arg);
            self.regs.cmd().write(cmd);
            if !self.wait_until_interrupt(1_000, || self.can_send_cmd()) {
                warn!("SD/MMC clock update timed out");
                return None;
            }
            return Some(self.regs.resp().read());
        }

        let stale = self.regs.rintsts().read();
        self.regs.rintsts().write(stale);
        let interrupt = self.interrupt.unwrap();

        if xfer.is_some() {
            // RXDR/TXDR require prompt FIFO service. Sleeping until the driver
            // task is scheduled again can trigger the controller's host
            // starvation timeout under load. Until DMA is available, keep the
            // requesting CPU active for the short data phase and use the raw
            // status register as the completion source.
            interrupt.finish();
            self.regs.ctrl().update(|r| r.with_int_enable(false));
            self.regs.cmdarg().write(arg);
            self.regs.cmd().write(cmd);

            let word_bytes = self.fifo_word_bytes();
            let fifo_locations = word_bytes.div_ceil(4);
            let transfer_len = self.regs.bytcnt().read() as usize;
            let mut offset = 0usize;
            let mut command_done = false;
            let mut data_done = false;
            let deadline = interrupt
                .current_ms()
                .saturating_add(Self::COMMAND_TIMEOUT_MS);

            loop {
                let status = self.regs.rintsts().read();
                command_done |= status.command_done();
                data_done |= status.data_transfer_over();

                if let Some(ref mut xfer) = xfer {
                    match xfer {
                        DataXfer::Read(buf)
                            if status.receive_fifo_data_request()
                                || status.data_transfer_over() =>
                        {
                            while self.fifo_cnt() >= fifo_locations
                                && offset + word_bytes <= transfer_len
                            {
                                self.read_fifo_word(&mut buf[offset..offset + word_bytes]);
                                offset += word_bytes;
                            }
                        }
                        DataXfer::Write(buf) if status.transmit_fifo_data_request() => {
                            while self.fifo_cnt() < 120 && offset + word_bytes <= transfer_len {
                                self.write_fifo_word(&buf[offset..offset + word_bytes]);
                                offset += word_bytes;
                            }
                        }
                        _ => {}
                    }
                }

                if status.into_bits() != 0 {
                    self.regs.rintsts().write(status);
                }

                if status.error() {
                    warn!(
                        "SD/MMC CMD{} failed: {status:?}, resp={:?}",
                        cmd.cmd_index(),
                        self.regs.resp().read()
                    );
                    interrupt.rearm();
                    return None;
                }

                if command_done && data_done {
                    let expected = transfer_len.min(match xfer.as_ref().unwrap() {
                        DataXfer::Read(buf) => buf.len(),
                        DataXfer::Write(buf) => buf.len(),
                    });
                    interrupt.rearm();
                    if offset != expected {
                        warn!(
                            "SD/MMC CMD{} transferred {offset}/{expected} bytes",
                            cmd.cmd_index()
                        );
                        return None;
                    }
                    return Some(self.regs.resp().read());
                }

                if interrupt.current_ms() >= deadline {
                    warn!(
                        "SD/MMC CMD{} PIO timed out: rintsts={:?}, status={:?}",
                        cmd.cmd_index(),
                        self.regs.rintsts().read(),
                        self.regs.status().read(),
                    );
                    interrupt.rearm();
                    return None;
                }
                core::hint::spin_loop();
            }
        }

        interrupt.prepare();
        interrupt.rearm();
        self.regs.cmdarg().write(arg);
        self.regs.cmd().write(cmd);
        let deadline = interrupt
            .current_ms()
            .saturating_add(Self::COMMAND_TIMEOUT_MS);

        loop {
            let now = interrupt.current_ms();
            if now >= deadline
                || !interrupt.wait(Duration::from_millis(deadline.saturating_sub(now)))
            {
                warn!(
                    "SD/MMC CMD{} interrupt timed out: rintsts={:?}, mintsts={:?}, status={:?}",
                    cmd.cmd_index(),
                    self.regs.rintsts().read(),
                    self.regs.mintsts().read(),
                    self.regs.status().read(),
                );
                interrupt.finish();
                interrupt.rearm();
                return None;
            }

            let events = interrupt.take_status();
            let status = RIntSts::from_bits(events & 0xffff);
            let failed = status.error();
            interrupt.rearm();

            if failed {
                warn!(
                    "SD/MMC CMD{} failed: {status:?}, resp={:?}",
                    cmd.cmd_index(),
                    self.regs.resp().read()
                );
                interrupt.finish();
                return None;
            }
            if status.command_done() {
                interrupt.finish();
                return Some(self.regs.resp().read());
            }
        }
    }

    fn reset_idmac(&self) -> bool {
        self.regs.ctrl().update(|r| r.with_dma_reset(true));
        if !self.wait_until_bounded(Self::RESET_TIMEOUT_MS, || {
            !self.regs.ctrl().read().dma_reset()
        }) {
            return false;
        }
        self.regs.bmod().update(|r| r.with_de(false).with_swr(true));
        self.wait_until_bounded(Self::RESET_TIMEOUT_MS, || !self.regs.bmod().read().swr())
    }

    fn stop_idmac(&self, fifo_threshold: u32) {
        self.regs.idinten().write(0);
        self.regs.bmod().update(|r| r.with_de(false));
        self.regs
            .ctrl()
            .update(|r| r.with_dma_enable(false).with_use_internal_dmac(false));
        self.regs.fifoth().write(fifo_threshold.into());
        let idmac = self.regs.idsts().read() & idmac::INT_CLEAR;
        if idmac != 0 {
            self.regs.idsts().write(idmac);
        }
        let controller = self.regs.rintsts().read();
        if controller.into_bits() != 0 {
            self.regs.rintsts().write(controller);
        }
        if let Some(interrupt) = self.interrupt {
            interrupt.finish();
            interrupt.rearm();
        }
    }

    fn send_cmd_dma(&mut self, command: Command<'_>) -> bool {
        let (Some(dma), Some(interrupt)) = (self.dma, self.interrupt) else {
            return false;
        };
        let (cmd, arg, Some(xfer)) = command.build() else {
            return false;
        };
        let (transfer_len, write) = match &xfer {
            DataXfer::Read(buf) => (buf.len(), false),
            DataXfer::Write(buf) => (buf.len(), true),
        };
        if transfer_len == 0
            || !transfer_len.is_multiple_of(Self::BLOCK_SIZE)
            || transfer_len > dma.max_transfer_len()
        {
            return false;
        }
        if !self.wait_until_interrupt(1_000, || self.can_send_cmd())
            || !self.wait_until_interrupt(1_000, || self.can_send_data())
        {
            warn!(
                "SD/MMC controller stayed busy before DMA CMD{}",
                cmd.cmd_index()
            );
            return false;
        }

        // SAFETY: DmaConfig guarantees exclusive static memory, and SdMmc is
        // exclusively borrowed for this entire transfer.
        let bounce =
            unsafe { core::slice::from_raw_parts_mut(dma.buffer as *mut u8, transfer_len) };
        if let DataXfer::Write(buf) = &xfer {
            bounce.copy_from_slice(buf);
        }
        let table_phys = (dma.virt_to_phys)(dma.descriptors);
        let buffer_phys = (dma.virt_to_phys)(dma.buffer);
        // SAFETY: DmaConfig guarantees an aligned descriptor region and
        // max_transfer_len bounds the number of descriptors written below.
        let descriptors = unsafe {
            core::slice::from_raw_parts_mut(
                dma.descriptors as *mut Descriptor,
                dma.descriptor_count,
            )
        };
        let Some(descriptor_count) =
            idmac::prepare_descriptors(descriptors, table_phys, buffer_phys, transfer_len)
        else {
            warn!("SD/MMC IDMAC memory is outside its 32-bit address domain");
            return false;
        };

        let fifo_threshold = self.regs.fifoth().read().into_bits();
        if !self.reset_idmac() {
            warn!("SD/MMC IDMAC reset timed out");
            return false;
        }
        let controller = self.regs.rintsts().read();
        self.regs.rintsts().write(controller);
        self.regs.idsts().write(idmac::INT_CLEAR);
        self.set_transaction_size(Self::BLOCK_SIZE as u16, transfer_len as u32);
        self.regs.fifoth().write(Self::IDMAC_FIFO_THRESHOLD.into());

        interrupt.prepare();
        (dma.barrier)();
        self.regs.dbaddr().write(table_phys as u32);
        self.regs.ctrl().update(|r| {
            r.with_use_internal_dmac(true)
                .with_dma_enable(true)
                .with_int_enable(true)
        });
        self.regs.idinten().write(idmac::INT_ENABLE);
        self.regs
            .bmod()
            .update(|r| r.with_swr(false).with_fb(true).with_de(true));
        (dma.barrier)();
        self.regs.pldmnd().write(1);
        interrupt.rearm();
        self.regs.cmdarg().write(arg);
        self.regs.cmd().write(cmd);

        let deadline = interrupt
            .current_ms()
            .saturating_add(Self::COMMAND_TIMEOUT_MS);
        let mut command_done = false;
        let mut controller_done = false;
        let mut idmac_done = false;
        let mut success = false;
        loop {
            let now = interrupt.current_ms();
            if now >= deadline
                || !interrupt.wait(Duration::from_millis(deadline.saturating_sub(now)))
            {
                warn!(
                    "SD/MMC CMD{} IDMAC timeout: cmd={} dto={} dma={} rint={:#010x} id={:#010x} \
                     status={:#010x} tcb={} tbb={} desc={:#010x} buf={:#010x}",
                    cmd.cmd_index(),
                    command_done,
                    controller_done,
                    idmac_done,
                    self.regs.rintsts().read().into_bits(),
                    self.regs.idsts().read(),
                    self.regs.status().read().into_bits(),
                    self.regs.tcbcnt().read(),
                    self.regs.tbbcnt().read(),
                    self.regs.dscaddr().read(),
                    self.regs.bufaddr().read(),
                );
                break;
            }

            let events = interrupt.take_status();
            let status = RIntSts::from_bits(events & 0xffff);
            command_done |= status.command_done();
            controller_done |= status.data_transfer_over();
            idmac_done |= events & IDMAC_COMPLETE != 0;
            if status.error() || events & IDMAC_ERROR != 0 {
                warn!(
                    "SD/MMC CMD{} IDMAC failed: events={events:#010x} resp={:?}",
                    cmd.cmd_index(),
                    self.regs.resp().read(),
                );
                break;
            }
            if command_done && controller_done && idmac_done {
                success = true;
                break;
            }
            interrupt.rearm();
        }

        (dma.barrier)();
        self.stop_idmac(fifo_threshold);
        if success && !write {
            if let DataXfer::Read(buf) = xfer {
                buf.copy_from_slice(bounce);
            } else {
                return false;
            }
        }
        if success && !self.dma_reported {
            info!(
                "SD/MMC IDMAC transfer verified: CMD{} blocks={} descriptors={descriptor_count}",
                cmd.cmd_index(),
                transfer_len / Self::BLOCK_SIZE,
            );
            self.dma_reported = true;
        }
        success
    }

    fn recover_idmac(&self) -> bool {
        self.regs.idinten().write(0);
        self.regs.bmod().update(|r| r.with_de(false));
        self.regs.ctrl().update(|r| {
            r.with_int_enable(false)
                .with_dma_enable(false)
                .with_use_internal_dmac(false)
                .with_fifo_reset(true)
                .with_dma_reset(true)
        });
        let reset_done = self.wait_until_bounded(Self::RESET_TIMEOUT_MS, || {
            let ctrl = self.regs.ctrl().read();
            !ctrl.fifo_reset() && !ctrl.dma_reset()
        });
        self.regs.bmod().update(|r| r.with_swr(true));
        let bmod_done =
            self.wait_until_bounded(Self::RESET_TIMEOUT_MS, || !self.regs.bmod().read().swr());
        self.regs.idsts().write(idmac::INT_CLEAR);
        let status = self.regs.rintsts().read();
        self.regs.rintsts().write(status);
        self.configure_timeouts();
        self.configure_interrupt_mask();
        let Some(interrupt) = self.interrupt else {
            return false;
        };
        interrupt.finish();
        interrupt.rearm();
        reset_done && bmod_done
    }

    fn stop_transmission(&self) -> bool {
        if self.send_cmd(Command::StopTransmission).is_none() {
            return false;
        }
        if self.wait_until_bounded(Self::COMMAND_TIMEOUT_MS, || self.can_send_data()) {
            true
        } else {
            warn!("SD/MMC CMD12 busy phase timed out");
            false
        }
    }

    fn init(&mut self) {
        info!("Initializing SD/MMC driver at {:?}", self.regs);

        trace!("ctrl: {:?}", self.regs.ctrl().read());
        trace!("pwren: {:?}", self.regs.pwren().read());
        trace!("clkdiv: {:?}", self.regs.clkdiv().read());
        trace!("clksrc: {:?}", self.regs.clksrc().read());
        trace!("clkena: {:?}", self.regs.clkena().read());
        trace!("tmout: {:?}", self.regs.tmout().read());
        trace!("ctype: {:?}", self.regs.ctype().read());
        trace!("cdetect: {:?}", self.regs.cdetect().read());
        trace!("wrtprt: {:?}", self.regs.wrtprt().read());
        trace!("usrid: {:?}", self.regs.usrid().read());
        trace!("verid: {:?}", self.regs.verid().read());
        trace!("hcon: {:?}", self.regs.hcon().read());
        trace!("uhs: {:?}", self.regs.uhs().read());
        trace!("bmod: {:?}", self.regs.bmod().read());
        trace!("dbaddr: {:?}", self.regs.dbaddr().read());

        // reset clock
        self.regs.clkena().write(ClkEna::new());
        self.send_cmd(Command::ResetClock);

        // set clock divider to 400kHz (low)
        self.regs.clkdiv().write(ClkDiv::new().with_clk_divider0(4));

        // enable clock
        self.regs.clkena().write(ClkEna::new().with_cclk_enable(1));
        self.send_cmd(Command::ResetClock);

        trace!("clock reset");

        // set data width -> 1bit
        self.regs.ctype().write(0.into());

        // reset dma
        self.regs.bmod().update(|r| r.with_de(false).with_swr(true));
        self.regs
            .ctrl()
            .update(|r| r.with_dma_reset(true).with_use_internal_dmac(false));

        self.configure_timeouts();

        if self.interrupt.is_some() {
            let stale = self.regs.rintsts().read();
            self.regs.rintsts().write(stale);
            self.configure_interrupt_mask();
            self.regs.ctrl().update(|r| r.with_int_enable(true));
            let interrupt = self.interrupt.unwrap();
            interrupt.finish();
            interrupt.rearm();
        }

        trace!("dma reset");

        trace!("ctrl: {:?}", self.regs.ctrl().read());

        self.send_cmd(Command::GoIdleState);
        trace!("idle state set");

        let resp = self.send_cmd(Command::SendIfCond(0x1aa)).unwrap();
        assert_eq!(resp[0] & 0xff, 0xaa, "unsupported version");

        loop {
            self.send_cmd(Command::AppCmd(0));
            let resp = self.send_cmd(Command::SdSendOpCond(0x41FF_8000)).unwrap();
            let ocr = resp[0];
            if ocr & 0x8000_0000 != 0 {
                info!("SD card is ready");
                if ocr & 0x4000_0000 != 0 {
                    debug!("SD card supports high capacity");
                } else {
                    debug!("SD card is standard capacity");
                }
                break;
            }

            trace!("SD card not ready, ocr: {ocr:x}");
            core::hint::spin_loop();
        }

        let resp = self.send_cmd(Command::AllSendCid).unwrap();
        let cid = unsafe { core::mem::transmute::<[u32; 4], Cid>(resp) };
        info!("cid: {cid:?}");

        let resp = self.send_cmd(Command::SendRelativeAddr).unwrap();
        let rca = (resp[0] >> 16) & 0xffff;
        debug!("rca: {rca:#x}");

        let resp = self.send_cmd(Command::SendCsd(rca << 16)).unwrap();
        let csd = unsafe { core::mem::transmute::<[u32; 4], CsdV2>(resp) };
        debug!("csd: {csd:?}");

        self.num_blocks = csd.num_blocks();
        info!("SD card capacity: {:#x} blocks", self.num_blocks);

        self.send_cmd(Command::SelectCard(rca << 16)).unwrap();

        self.send_cmd(Command::AppCmd(rca << 16)).unwrap();

        self.set_transaction_size(8, 8);
        let mut buf = [0u8; 512];
        self.send_cmd(Command::SendScr(&mut buf)).unwrap();

        trace!("fifo count: {}", self.fifo_cnt());
        let resp = u64::from_le_bytes(buf[..8].try_into().unwrap());
        debug!("Bus width supported: {:#x?}", (resp >> 8) & 0xf);
        trace!("fifo count: {}", self.fifo_cnt());

        trace!("ctrl: {:?}", self.regs.ctrl().read());
        let rintsts = self.regs.rintsts().read();
        trace!("rintsts: {rintsts:?}");
        self.regs.rintsts().write(rintsts); // clear interrupt status

        info!("SD/MMC driver initialized");
    }

    /// Reads a single block from the SD/MMC card.
    pub fn read_block(&mut self, block: u32, buf: &mut [u8; 512]) -> Result<(), SdMmcError> {
        if self.dma.is_some() {
            return self.read_blocks(block, buf);
        }
        self.set_transaction_size(512, 512);
        for attempt in 1..=Self::BLOCK_ATTEMPTS {
            if self
                .send_cmd(Command::ReadSingleBlock(block, &mut buf[..]))
                .is_some()
            {
                trace!("fifo count: {}", self.fifo_cnt());
                return Ok(());
            }

            warn!("SD/MMC block {block} read attempt {attempt} failed");
            if !self.recover_data_path() {
                return Err(SdMmcError::RecoveryFailed);
            }
            self.set_transaction_size(512, 512);
        }
        Err(SdMmcError::CommandFailed)
    }

    /// Writes a single block to the SD/MMC card.
    pub fn write_block(&mut self, block: u32, buf: &[u8; 512]) -> Result<(), SdMmcError> {
        if self.dma.is_some() {
            return self.write_blocks(block, buf);
        }
        self.set_transaction_size(512, 512);
        for attempt in 1..=Self::BLOCK_ATTEMPTS {
            if self
                .send_cmd(Command::WriteSingleBlock(block, &buf[..]))
                .is_some()
            {
                trace!("fifo count: {}", self.fifo_cnt());
                return Ok(());
            }

            warn!("SD/MMC block {block} write attempt {attempt} failed");
            if !self.recover_data_path() {
                return Err(SdMmcError::RecoveryFailed);
            }
            self.set_transaction_size(512, 512);
        }
        Err(SdMmcError::CommandFailed)
    }

    /// Reads one or more contiguous blocks, using IDMAC when configured.
    pub fn read_blocks(&mut self, block: u32, buf: &mut [u8]) -> Result<(), SdMmcError> {
        if buf.is_empty() || !buf.len().is_multiple_of(Self::BLOCK_SIZE) {
            return Err(SdMmcError::InvalidParameter);
        }
        let block_count = u32::try_from(buf.len() / Self::BLOCK_SIZE)
            .map_err(|_| SdMmcError::InvalidParameter)?;
        if block.checked_add(block_count).is_none() {
            return Err(SdMmcError::InvalidParameter);
        }
        let Some(dma) = self.dma else {
            let (blocks, _) = buf.as_chunks_mut::<{ Self::BLOCK_SIZE }>();
            for (offset, data) in blocks.iter_mut().enumerate() {
                self.read_block(block + offset as u32, data)?;
            }
            return Ok(());
        };
        if buf.len() > dma.max_transfer_len() {
            return Err(SdMmcError::InvalidParameter);
        }

        for attempt in 1..=Self::BLOCK_ATTEMPTS {
            let command = if block_count == 1 {
                Command::ReadSingleBlock(block, buf)
            } else {
                Command::ReadMultipleBlocks(block, buf)
            };
            let data_ok = self.send_cmd_dma(command);
            let stop_ok = block_count == 1 || self.stop_transmission();
            if data_ok && stop_ok {
                return Ok(());
            }
            warn!(
                "SD/MMC blocks {block}..{} DMA read attempt {attempt} failed",
                block + block_count,
            );
            if !self.recover_idmac() {
                return Err(SdMmcError::RecoveryFailed);
            }
        }
        Err(SdMmcError::CommandFailed)
    }

    /// Writes one or more contiguous blocks, using IDMAC when configured.
    pub fn write_blocks(&mut self, block: u32, buf: &[u8]) -> Result<(), SdMmcError> {
        if buf.is_empty() || !buf.len().is_multiple_of(Self::BLOCK_SIZE) {
            return Err(SdMmcError::InvalidParameter);
        }
        let block_count = u32::try_from(buf.len() / Self::BLOCK_SIZE)
            .map_err(|_| SdMmcError::InvalidParameter)?;
        if block.checked_add(block_count).is_none() {
            return Err(SdMmcError::InvalidParameter);
        }
        let Some(dma) = self.dma else {
            let (blocks, _) = buf.as_chunks::<{ Self::BLOCK_SIZE }>();
            for (offset, data) in blocks.iter().enumerate() {
                self.write_block(block + offset as u32, data)?;
            }
            return Ok(());
        };
        if buf.len() > dma.max_transfer_len() {
            return Err(SdMmcError::InvalidParameter);
        }

        for attempt in 1..=Self::BLOCK_ATTEMPTS {
            let command = if block_count == 1 {
                Command::WriteSingleBlock(block, buf)
            } else {
                Command::WriteMultipleBlocks(block, buf)
            };
            let data_ok = self.send_cmd_dma(command);
            let stop_ok = block_count == 1 || self.stop_transmission();
            if data_ok && stop_ok {
                return Ok(());
            }
            warn!(
                "SD/MMC blocks {block}..{} DMA write attempt {attempt} failed",
                block + block_count,
            );
            if !self.recover_idmac() {
                return Err(SdMmcError::RecoveryFailed);
            }
        }
        Err(SdMmcError::CommandFailed)
    }

    /// Maximum number of blocks accepted by one request.
    pub fn max_transfer_blocks(&self) -> usize {
        self.dma
            .map_or(1, |dma| dma.max_transfer_len() / Self::BLOCK_SIZE)
    }

    /// Waits until the card has finished programming all previously written data.
    ///
    /// A successful data-transfer interrupt only means that the controller has
    /// handed the block to the card. The card may still hold DAT0 low while it
    /// programs flash. Filesystem `sync`/`fsync` must not complete before that
    /// busy phase ends.
    pub fn flush(&mut self) -> Result<(), SdMmcError> {
        if self.wait_until_bounded(Self::FLUSH_TIMEOUT_MS, || self.can_send_data()) {
            Ok(())
        } else {
            warn!(
                "SD/MMC flush timed out: rintsts={:?}, status={:?}",
                self.regs.rintsts().read(),
                self.regs.status().read(),
            );
            Err(SdMmcError::FlushTimeout)
        }
    }

    /// Returns the number of blocks.
    pub fn num_blocks(&self) -> u64 {
        self.num_blocks
    }

    /// The size of a block in bytes.
    pub const BLOCK_SIZE: usize = 512;
}

unsafe impl Send for SdMmc {}
unsafe impl Sync for SdMmc {}
