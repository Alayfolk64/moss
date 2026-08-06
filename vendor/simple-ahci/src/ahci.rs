use alloc::{alloc::alloc_zeroed, sync::Arc, vec::Vec};
use core::{
    alloc::Layout,
    marker::PhantomData,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
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
        CommandToken, CompletionQueue, NonNcqAdmission, completed_commands, port_interrupt_failed,
        transfer_in_range,
    },
    hal::wait_until_timeout,
    mmio::{
        AhciMmio, AhciMmioVolatileFieldAccess, GenericHostControlVolatileFieldAccess, ICC,
        PortRegisters, PortRegistersVolatileFieldAccess, PxCMD, PxI,
    },
    types::{
        AHCI_MAX_BYTES_PER_CMD, AHCI_MAX_BYTES_PER_SG, AHCI_MAX_SG, ahci_cmd_hdr, ahci_cmd_list,
        ahci_cmd_tbl, ahci_cmd_tblVolatileFieldAccess, ahci_rx_fis, ahci_sg, sata_fis_h2d,
    },
};

const COMMAND_TIMEOUT_MS: u64 = 1_000;

fn alloc<T: Sized>(align: usize) -> VolatilePtr<'static, T> {
    unsafe {
        VolatilePtr::new(NonNull::new_unchecked(
            alloc_zeroed(Layout::from_size_align(size_of::<T>(), align).unwrap()).cast(),
        ))
    }
}

struct AhciPort<H> {
    host: VolatilePtr<'static, AhciMmio>,
    port: VolatilePtr<'static, PortRegisters>,
    index: u8,
    cmd_list: VolatilePtr<'static, ahci_cmd_list>,
    #[allow(dead_code)]
    fis: VolatilePtr<'static, ahci_rx_fis>,
    cmd_tables: Vec<VolatilePtr<'static, ahci_cmd_tbl>>,
    completion: CompletionQueue,
    admission: NonNcqAdmission,
    issue_lock: Mutex<()>,
    recovering: AtomicBool,
    recovery_needed: AtomicBool,
    interrupt_mode: AtomicBool,
    _h: PhantomData<H>,
}

unsafe impl<H: Hal> Send for AhciPort<H> {}
unsafe impl<H: Hal> Sync for AhciPort<H> {}

impl<H: Hal> AhciPort<H> {
    fn try_new(host: VolatilePtr<'static, AhciMmio>, index: u8, slot_count: usize) -> Option<Self> {
        let port = unsafe {
            host.ports()
                .map(|ports| ports.cast::<PortRegisters>().add(index as usize))
        };

        port.IE().write(PxI::new());
        port.CMD().update(|cmd| cmd.with_ST(false).with_FRE(false));
        if !wait_until_timeout::<H>(|| !port.CMD().read().CR(), 500)
            || !wait_until_timeout::<H>(|| !port.CMD().read().FR(), 500)
        {
            warn!("AHCI port {index} failed to stop");
            return None;
        }

        let cap = host.host().cap().read();
        let tfd = port.TFD().read();
        if (tfd.STS_BSY() || tfd.STS_DRQ()) && cap.SCLO() {
            port.CMD().update(|cmd| cmd.with_CLO(true));
            if !wait_until_timeout::<H>(|| !port.CMD().read().CLO(), 1_000) {
                warn!("AHCI port {index} CLO timeout");
                return None;
            }
        }

        port.CMD().update(|cmd| cmd.with_SUD(true));
        if !wait_until_timeout::<H>(|| port.SSTS().read().DET() == 3, 1_000) {
            debug!("AHCI port {index} has no active SATA link");
            return None;
        }

        port.SERR().write(port.SERR().read());
        port.IS().write(port.IS().read());
        host.host().is().write(1_u32 << index);

        let cmd_list = alloc::<ahci_cmd_list>(1024);
        let cmd_list_addr = H::virt_to_phys(cmd_list.as_raw_ptr().addr().get());
        port.CLB().write(cmd_list_addr as u32);
        port.CLBU().write((cmd_list_addr >> 32) as u32);

        let fis = alloc::<ahci_rx_fis>(256);
        let fis_addr = H::virt_to_phys(fis.as_raw_ptr().addr().get());
        port.FB().write(fis_addr as u32);
        port.FBU().write((fis_addr >> 32) as u32);

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
        port.CMD().write(
            PxCMD::new()
                .with_ICC(ICC::Active)
                .with_FRE(true)
                .with_POD(true)
                .with_SUD(true)
                .with_ST(true),
        );
        if !wait_until_timeout::<H>(
            || {
                let tfd = port.TFD().read();
                !(tfd.STS_ERR() || tfd.STS_DRQ() || tfd.STS_BSY())
            },
            1_000,
        ) {
            warn!("AHCI port {index} start timeout: {:?}", port.TFD().read());
            return None;
        }

        Some(Self {
            host,
            port,
            index,
            cmd_list,
            fis,
            cmd_tables,
            completion: CompletionQueue::new(slot_count),
            admission: NonNcqAdmission::new(),
            issue_lock: Mutex::new(()),
            recovering: AtomicBool::new(false),
            recovery_needed: AtomicBool::new(false),
            interrupt_mode: AtomicBool::new(false),
            _h: PhantomData,
        })
    }

    fn enable_interrupts(&self) {
        self.port.IS().write(self.port.IS().read());
        self.host.host().is().write(1_u32 << self.index);
        self.port.IE().write(PxI::default_enable().with_DP(true));
        self.host.host().ghc().update(|ghc| ghc.with_IE(true));
        self.interrupt_mode.store(true, Ordering::Release);
    }

    fn reserve_slot(&self) -> Option<CommandToken> {
        let mut selected = None;
        let available = || {
            if self.recovering.load(Ordering::Acquire)
                || self.recovery_needed.load(Ordering::Acquire)
            {
                return false;
            }
            if !self.admission.try_acquire() {
                return false;
            }
            selected = self.completion.try_reserve();
            if selected.is_none() {
                self.admission.release();
            }
            selected.is_some()
        };
        let mut available = available;
        if !available() && !H::wait_for_event(COMMAND_TIMEOUT_MS, &mut available) {
            return None;
        }
        selected
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
        let table = &self.cmd_tables[token.slot];
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
            self.cmd_list
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

    fn issue(&self, token: CommandToken) {
        loop {
            let guard = self.issue_lock.lock();
            if self.recovering.load(Ordering::Acquire)
                || self.recovery_needed.load(Ordering::Acquire)
            {
                drop(guard);
                self.recover_if_needed();
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
            if self.interrupt_mode.load(Ordering::Acquire) && self.port.CI().read() & bit == 0 {
                let status_bits = self.port.IS().read().into_bits();
                let failed = port_interrupt_failed(status_bits) || self.port.TFD().read().STS_ERR();
                if failed {
                    self.recovery_needed.store(true, Ordering::Release);
                }
                self.completion.publish(bit, failed);
                H::notify_event();
            }
            return;
        }
    }

    fn finish_token(&self, token: CommandToken) -> bool {
        let done = || self.completion.is_done(token);

        let completed = if self.interrupt_mode.load(Ordering::Acquire) {
            H::wait_for_event(COMMAND_TIMEOUT_MS, done)
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
            self.recover_if_needed();
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
            self.recover_if_needed();
        }
        self.admission.release();
        H::notify_event();
        result == Some(true)
    }

    fn exec_cmd_buffer(
        &self,
        cfis: sata_fis_h2d,
        buffer_address: usize,
        buffer_len: usize,
        is_write: bool,
    ) -> bool {
        let Some(token) = self.reserve_slot() else {
            error!("AHCI command slot wait timed out");
            return false;
        };
        if !self.prepare_command(token, cfis, buffer_address, buffer_len, is_write) {
            self.completion.cancel(token);
            self.admission.release();
            H::notify_event();
            return false;
        }
        self.issue(token);
        self.finish_token(token)
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
        self.port.IE().write(PxI::new());
        self.port.IS().write(PxI::from_bits(status_bits));

        let active = self.completion.issued();
        let failed = port_interrupt_failed(status_bits);
        if failed {
            self.recovery_needed.store(true, Ordering::Release);
        }
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

    fn recover_if_needed(&self) {
        if self.recovering.load(Ordering::Acquire) {
            let _ = H::wait_for_event(COMMAND_TIMEOUT_MS, || {
                !self.recovering.load(Ordering::Acquire)
            });
            return;
        }
        if !self.recovery_needed.load(Ordering::Acquire) {
            return;
        }
        if self
            .recovering
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            let _ = H::wait_for_event(COMMAND_TIMEOUT_MS, || {
                !self.recovering.load(Ordering::Acquire)
            });
            return;
        }

        let _issue_guard = self.issue_lock.lock();
        if !self.recovery_needed.swap(false, Ordering::AcqRel) {
            self.recovering.store(false, Ordering::Release);
            H::notify_event();
            return;
        }

        self.port.IE().write(PxI::new());
        let active = self.completion.issued();
        self.completion.publish(active, true);

        self.port.CMD().update(|cmd| cmd.with_ST(false));
        let _ = wait_until_timeout::<H>(|| !self.port.CMD().read().CR(), 500);
        self.port.CMD().update(|cmd| cmd.with_FRE(false));
        let _ = wait_until_timeout::<H>(|| !self.port.CMD().read().FR(), 500);
        self.port.SERR().write(self.port.SERR().read());
        self.port.IS().write(self.port.IS().read());
        self.host.host().is().write(1_u32 << self.index);
        self.port
            .CMD()
            .update(|cmd| cmd.with_FRE(true).with_ST(true));
        if self.interrupt_mode.load(Ordering::Acquire) {
            self.port.IE().write(PxI::default_enable().with_DP(true));
        }
        // The reset also covers failures reported by commands that were active
        // when recovery began.
        self.recovery_needed.store(false, Ordering::Release);
        self.recovering.store(false, Ordering::Release);
        H::notify_event();
    }
}

struct AhciInner<H> {
    _mmio: VolatilePtr<'static, AhciMmio>,
    port: AhciPort<H>,
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
        let mmio = unsafe { VolatilePtr::new(NonNull::new(base as *mut _).unwrap()) };
        let host = mmio.host();
        host.ghc().update(|ghc| ghc.with_IE(false).with_HR(true));
        if !wait_until_timeout::<H>(|| !host.ghc().read().HR(), 1_000) {
            error!("AHCI HBA reset timeout");
            return None;
        }
        host.ghc().update(|ghc| ghc.with_AE(true).with_IE(false));

        let cap = host.cap().read();
        let pi = host.pi().read();
        info!("AHCI ver {}, cap {}, ports={pi:#x}", host.vs().read(), cap);
        let slot_count = usize::from(cap.NCS()) + 1;
        let mut selected = None;
        for index in 0..=cap.NP() {
            if pi & (1_u32 << index) != 0 {
                if let Some(port) = AhciPort::<H>::try_new(mmio, index, slot_count) {
                    selected = Some(port);
                    break;
                }
            }
        }
        let port = selected?;
        Some(PreparedAhci {
            inner: Arc::new(AhciInner { _mmio: mmio, port }),
        })
    }

    /// Compatibility constructor for environments without a registered IRQ.
    pub unsafe fn try_new(base: usize) -> Option<Self> {
        unsafe { Self::prepare(base) }?.identify()
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
