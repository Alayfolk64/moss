//! LS2K1000 integrated devices discovered from the firmware device tree.

use alloc::{boxed::Box, format, string::String, vec, vec::Vec};
use core::{
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
    time::Duration,
};

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use axdriver_block::{
    BlockDriverOps,
    ahci::{AhciDriver, AhciIrqHandle},
};
use axdriver_net::dwmac_ls2k1000::{DwmacConfig, DwmacHal, DwmacLs2k1000};
use fdt_parser::{Fdt, Node, Status};

use crate::{
    AxDeviceEnum,
    drivers::DriverProbe,
    ls2k1000_irq::{LIOINTC_IRQS_PER_CONTROLLER, translated_liointc_irq},
};

const UNCACHED_DMW_BASE: usize = 0x8000_0000_0000_0000;
const PHYS_ADDR_MASK: usize = (1usize << 48) - 1;

fn enabled(node: &Node<'_>) -> bool {
    node.status() != Some(Status::Disabled)
}

fn compatible(node: &Node<'_>, choices: &[&str]) -> bool {
    node.compatibles()
        .any(|actual| choices.iter().any(|wanted| actual == *wanted))
}

fn uncached_addr(paddr: usize) -> usize {
    UNCACHED_DMW_BASE | (paddr & PHYS_ADDR_MASK)
}

fn first_reg(node: &Node<'_>) -> Option<(usize, usize)> {
    node.reg()?.next().map(|reg| {
        (
            reg.address as usize,
            reg.size.unwrap_or(core::mem::size_of::<u32>()),
        )
    })
}

fn first_interrupt(node: &Node<'_>) -> Option<usize> {
    node.interrupts()?.next()?.next().map(|irq| irq as usize)
}

fn translated_irq(fdt: &Fdt<'_>, node: &Node<'_>) -> Option<usize> {
    let parent = node.interrupt_parent()?.node.phandle()?;
    let source = first_interrupt(node)?;
    if source >= LIOINTC_IRQS_PER_CONTROLLER {
        return None;
    }
    fdt.find_compatible(&[
        "loongson,2k1000-icu",
        "loongson,ls2k1000-icu",
        "loongson,liointc",
    ])
    .filter(enabled)
    .position(|controller| controller.phandle() == Some(parent))
    .and_then(|index| translated_liointc_irq(index, source))
}

fn dma_mask(node: &Node<'_>) -> Option<u64> {
    let property = node.find_property("dma-mask")?;
    match property.raw_value().len() {
        4 => Some(property.u32() as u64),
        n if n >= 8 => Some(property.u64()),
        _ => None,
    }
}

struct AhciHalImpl;

static AHCI_WAIT_QUEUE: axtask::WaitQueue = axtask::WaitQueue::new();
static AHCI_IRQ_HANDLE: AtomicPtr<AhciIrqHandle<AhciHalImpl>> = AtomicPtr::new(ptr::null_mut());

impl axdriver_block::ahci::AhciHal for AhciHalImpl {
    fn virt_to_phys(virtual_address: usize) -> usize {
        axhal::mem::virt_to_phys(virtual_address.into()).as_usize()
    }

    fn current_ms() -> u64 {
        axhal::time::monotonic_time_nanos() / 1_000_000
    }

    fn flush_dcache() {
        unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };
    }

    fn wait_for_event<F>(timeout_ms: u64, condition: F) -> bool
    where
        F: FnMut() -> bool,
    {
        !AHCI_WAIT_QUEUE.wait_timeout_until(Duration::from_millis(timeout_ms), condition)
    }

    fn notify_event() {
        AHCI_WAIT_QUEUE.notify_all(false);
    }
}

type RawAhci = AhciDriver<AhciHalImpl>;

fn handle_ahci_interrupt() {
    let handle = AHCI_IRQ_HANDLE.load(Ordering::Acquire);
    if !handle.is_null() {
        unsafe { &*handle }.handle_interrupt();
    }
}

#[derive(Clone, Debug)]
struct Partition {
    number: usize,
    start_lba: u64,
    blocks: u64,
    partuuid: String,
    label: String,
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes[..8].try_into().unwrap())
}

fn read_bytes(dev: &RawAhci, offset: u64, output: &mut [u8]) -> DevResult {
    let block_size = dev.block_size();
    let mut done = 0;
    while done < output.len() {
        let absolute = offset + done as u64;
        let lba = absolute / block_size as u64;
        let within = absolute as usize % block_size;
        let mut block = vec![0u8; block_size];
        dev.read_block(lba, &mut block)?;
        let count = (block_size - within).min(output.len() - done);
        output[done..done + count].copy_from_slice(&block[within..within + count]);
        done += count;
    }
    Ok(())
}

fn guid_string(bytes: &[u8]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:\
         02x}{:02x}{:02x}",
        bytes[3],
        bytes[2],
        bytes[1],
        bytes[0],
        bytes[5],
        bytes[4],
        bytes[7],
        bytes[6],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn gpt_partitions(dev: &RawAhci) -> DevResult<Vec<Partition>> {
    let block_size = dev.block_size();
    let mut header = vec![0u8; block_size];
    dev.read_block(1, &mut header)?;
    if header.get(..8) != Some(b"EFI PART") {
        return Ok(Vec::new());
    }
    let entries_lba = le_u64(&header[72..80]);
    let count = le_u32(&header[80..84]) as usize;
    let entry_size = le_u32(&header[84..88]) as usize;
    if !(128..=1024).contains(&entry_size) || count > 4096 {
        return Err(DevError::BadState);
    }

    let mut partitions = Vec::new();
    for index in 0..count {
        let mut entry = vec![0u8; entry_size];
        let offset = entries_lba
            .checked_mul(block_size as u64)
            .and_then(|base| base.checked_add((index * entry_size) as u64))
            .ok_or(DevError::BadState)?;
        read_bytes(dev, offset, &mut entry)?;
        if entry[..16].iter().all(|byte| *byte == 0) {
            continue;
        }
        let first = le_u64(&entry[32..40]);
        let last = le_u64(&entry[40..48]);
        if last < first || last >= dev.num_blocks() {
            return Err(DevError::BadState);
        }
        let mut label = String::new();
        for pair in entry[56..entry_size.min(128)].chunks_exact(2) {
            let value = u16::from_le_bytes([pair[0], pair[1]]);
            if value == 0 {
                break;
            }
            if let Some(character) = char::from_u32(value as u32) {
                label.push(character);
            }
        }
        partitions.push(Partition {
            number: index + 1,
            start_lba: first,
            blocks: last - first + 1,
            partuuid: guid_string(&entry[16..32]),
            label,
        });
    }
    Ok(partitions)
}

fn partitions(dev: &RawAhci) -> DevResult<Vec<Partition>> {
    let mut sector = vec![0u8; dev.block_size()];
    if sector.len() < 512 {
        return Err(DevError::Unsupported);
    }
    dev.read_block(0, &mut sector)?;
    if sector[510..512] != [0x55, 0xaa] {
        return Ok(Vec::new());
    }
    if (0..4).any(|index| sector[446 + index * 16 + 4] == 0xee) {
        return gpt_partitions(dev);
    }

    let signature = le_u32(&sector[440..444]);
    let mut result = Vec::new();
    for index in 0..4 {
        let entry = &sector[446 + index * 16..446 + (index + 1) * 16];
        let kind = entry[4];
        let start = le_u32(&entry[8..12]) as u64;
        let blocks = le_u32(&entry[12..16]) as u64;
        if kind == 0 || blocks == 0 {
            continue;
        }
        if start >= dev.num_blocks() || blocks > dev.num_blocks() - start {
            return Err(DevError::BadState);
        }
        result.push(Partition {
            number: index + 1,
            start_lba: start,
            blocks,
            partuuid: format!("{signature:08x}-{:02x}", index + 1),
            label: String::new(),
        });
    }
    Ok(result)
}

fn is_ext4(dev: &RawAhci, start_lba: u64) -> bool {
    let offset = match start_lba
        .checked_mul(dev.block_size() as u64)
        .and_then(|value| value.checked_add(1024 + 56))
    {
        Some(offset) => offset,
        None => return false,
    };
    let mut magic = [0u8; 2];
    read_bytes(dev, offset, &mut magic).is_ok() && magic == [0x53, 0xef]
}

fn root_argument() -> Option<&'static str> {
    axhal::dtb::get_chosen_bootargs()?
        .split_ascii_whitespace()
        .find_map(|argument| argument.strip_prefix("root="))
}

fn select_partition(dev: &RawAhci) -> DevResult<(u64, u64, Option<usize>)> {
    let available = partitions(dev)?;
    if let Some(root) = root_argument() {
        if root == "/dev/sda" {
            return is_ext4(dev, 0)
                .then_some((0, dev.num_blocks(), None))
                .ok_or(DevError::BadState);
        }
        if let Some(number) = root
            .strip_prefix("/dev/sda")
            .and_then(|number| number.parse::<usize>().ok())
        {
            let partition = available
                .iter()
                .find(|partition| partition.number == number)
                .ok_or(DevError::InvalidParam)?;
            return is_ext4(dev, partition.start_lba)
                .then_some((
                    partition.start_lba,
                    partition.blocks,
                    Some(partition.number),
                ))
                .ok_or(DevError::BadState);
        }
        if let Some(partuuid) = root.strip_prefix("PARTUUID=") {
            let partition = available
                .iter()
                .find(|partition| partition.partuuid.eq_ignore_ascii_case(partuuid))
                .ok_or(DevError::InvalidParam)?;
            return is_ext4(dev, partition.start_lba)
                .then_some((
                    partition.start_lba,
                    partition.blocks,
                    Some(partition.number),
                ))
                .ok_or(DevError::BadState);
        }
        if let Some(label) = root.strip_prefix("PARTLABEL=") {
            let partition = available
                .iter()
                .find(|partition| partition.label == label)
                .ok_or(DevError::InvalidParam)?;
            return is_ext4(dev, partition.start_lba)
                .then_some((
                    partition.start_lba,
                    partition.blocks,
                    Some(partition.number),
                ))
                .ok_or(DevError::BadState);
        }
        return Err(DevError::Unsupported);
    }

    let candidates = available
        .iter()
        .filter(|partition| is_ext4(dev, partition.start_lba))
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [partition] => Ok((
            partition.start_lba,
            partition.blocks,
            Some(partition.number),
        )),
        [] if is_ext4(dev, 0) => Ok((0, dev.num_blocks(), None)),
        _ => Err(DevError::BadState),
    }
}

pub struct Ls2kAhciDevice {
    inner: RawAhci,
    start_lba: u64,
    blocks: u64,
}

impl BaseDriverOps for Ls2kAhciDevice {
    fn device_name(&self) -> &str {
        "ls2k1000-ahci-root"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl BlockDriverOps for Ls2kAhciDevice {
    fn num_blocks(&self) -> u64 {
        self.blocks
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn read_block(&self, block_id: u64, buffer: &mut [u8]) -> DevResult {
        let count = buffer.len().div_ceil(self.block_size()) as u64;
        if block_id >= self.blocks || count > self.blocks - block_id {
            return Err(DevError::InvalidParam);
        }
        self.inner.read_block(self.start_lba + block_id, buffer)
    }

    fn write_block(&self, block_id: u64, buffer: &[u8]) -> DevResult {
        let count = buffer.len().div_ceil(self.block_size()) as u64;
        if block_id >= self.blocks || count > self.blocks - block_id {
            return Err(DevError::InvalidParam);
        }
        self.inner.write_block(self.start_lba + block_id, buffer)
    }

    fn flush(&self) -> DevResult {
        self.inner.flush()
    }
}

pub struct Ls2kAhciDriver;

impl DriverProbe for Ls2kAhciDriver {
    fn probe_global() -> Option<AxDeviceEnum> {
        let fdt = axhal::dtb::get_fdt().expect("LS2K1000 AHCI requires a DTB");
        let node = fdt
            .find_compatible(&[
                "loongson,ls-ahci",
                "loongson,ls2k1000-ahci",
                "loongson,2k1000-ahci",
            ])
            .find(enabled)?;
        let (paddr, size) = first_reg(&node).expect("enabled AHCI node has no reg property");
        assert!(size >= 0x1100, "AHCI register window is too small");
        let irq = translated_irq(fdt, &node)
            .expect("enabled LS2K1000 AHCI node requires a translated DT IRQ");
        let mask = dma_mask(&node).unwrap_or(u32::MAX as u64);
        assert!(
            mask >= u32::MAX as u64,
            "AHCI DMA mask is narrower than 32 bits"
        );
        let prepared = unsafe { RawAhci::prepare(uncached_addr(paddr)) }
            .expect("failed to prepare DT-discovered LS2K1000 AHCI");
        let handle = Box::into_raw(Box::new(prepared.irq_handle()));
        if AHCI_IRQ_HANDLE
            .compare_exchange(ptr::null_mut(), handle, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            drop(unsafe { Box::from_raw(handle) });
            panic!("only one LS2K1000 AHCI controller is supported");
        }
        if !axhal::irq::register(irq, handle_ahci_interrupt) {
            AHCI_IRQ_HANDLE.store(ptr::null_mut(), Ordering::Release);
            drop(unsafe { Box::from_raw(handle) });
            panic!("failed to register LS2K1000 AHCI IRQ {irq}");
        }
        prepared.enable_interrupts();
        let inner = prepared
            .identify()
            .expect("failed to identify DT-discovered LS2K1000 AHCI disk");
        let (start_lba, blocks, partition) =
            select_partition(&inner).expect("root= does not select one usable AHCI ext4 partition");
        info!(
            "LS2K1000 AHCI from DTB: paddr={paddr:#x}, size={size:#x}, irq={irq}, \
             dma_mask={mask:#x}, root_partition={partition:?}, start_lba={start_lba}, \
             blocks={blocks}"
        );
        Some(AxDeviceEnum::from_block(Ls2kAhciDevice {
            inner,
            start_lba,
            blocks,
        }))
    }
}

pub struct GmacHalImpl;

impl DwmacHal for GmacHalImpl {
    fn virt_to_phys(virtual_address: usize) -> usize {
        axhal::mem::virt_to_phys(virtual_address.into()).as_usize()
    }

    fn current_ms() -> u64 {
        axhal::time::monotonic_time_nanos() / 1_000_000
    }

    fn dma_barrier() {
        unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };
    }
}

pub type Ls2kGmacDevice = DwmacLs2k1000<GmacHalImpl>;

fn gmac0_node<'a>(fdt: &'a Fdt<'a>) -> Option<Node<'a>> {
    if let Some(path) = fdt.find_aliase("ethernet0") {
        return fdt.find_nodes(path).find(|node| {
            enabled(node) && compatible(node, &["snps,dwmac-3.70a", "snps,arc-dwmac-3.70a"])
        });
    }
    fdt.find_compatible(&["snps,dwmac-3.70a", "snps,arc-dwmac-3.70a"])
        .find(enabled)
}

pub struct Ls2kGmacDriver;

impl DriverProbe for Ls2kGmacDriver {
    fn probe_global() -> Option<AxDeviceEnum> {
        let fdt = axhal::dtb::get_fdt().expect("LS2K1000 GMAC requires a DTB");
        let node = gmac0_node(fdt)?;
        let (paddr, size) = first_reg(&node).expect("enabled GMAC0 node has no reg property");
        assert!(size >= 0x102c, "GMAC0 register window is too small");
        let phy_mode = node
            .find_property("phy-mode")
            .map(|property| property.str())
            .unwrap_or("rgmii");
        assert!(phy_mode.starts_with("rgmii"), "unsupported GMAC0 phy-mode");
        let phy_address = node
            .find_property("phy-handle")
            .and_then(|property| fdt.get_node_by_phandle(property.u32().into()))
            .and_then(|phy| phy.reg()?.next().map(|reg| reg.address as u8))
            .unwrap_or(0);
        let mac_address = node
            .find_property("local-mac-address")
            .or_else(|| node.find_property("mac-address"))
            .and_then(|property| {
                property
                    .raw_value()
                    .get(..6)
                    .and_then(|mac| mac.try_into().ok())
            });
        let irq = translated_irq(fdt, &node);
        let mask = dma_mask(&node).unwrap_or(u32::MAX as u64);
        assert!(
            mask >= u32::MAX as u64,
            "GMAC0 DMA mask is narrower than 32 bits"
        );
        let device = unsafe {
            Ls2kGmacDevice::try_new(
                uncached_addr(paddr),
                DwmacConfig {
                    mac_address,
                    phy_address,
                    irq,
                },
            )
        }
        .expect("failed to initialize DT-discovered LS2K1000 GMAC0");
        let mut device = device;
        if let Some(irq) = irq {
            if axhal::irq::register(irq, axdriver_net::dwmac_ls2k1000::handle_interrupt) {
                device.enable_interrupt_mode();
            } else {
                warn!("LS2K1000 GMAC0 IRQ {irq} registration failed; using polling mode");
            }
        } else {
            warn!("LS2K1000 GMAC0 has no DT IRQ; using polling mode");
        }
        info!(
            "LS2K1000 GMAC0 from DTB: paddr={paddr:#x}, size={size:#x}, irq={irq:?}, \
             phy_mode={phy_mode}, phy_address={phy_address}, dma_mask={mask:#x}"
        );
        Some(AxDeviceEnum::from_net(device))
    }
}

#[cfg(test)]
mod tests {
    use super::guid_string;

    #[test]
    fn formats_gpt_guid_with_mixed_endian_fields() {
        assert_eq!(
            guid_string(&[
                0x78, 0x56, 0x34, 0x12, 0xbc, 0x9a, 0xf0, 0xde, 1, 2, 3, 4, 5, 6, 7, 8
            ]),
            "12345678-9abc-def0-0102-030405060708"
        );
    }
}
