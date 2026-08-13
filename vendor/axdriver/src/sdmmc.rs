//! SD/MMC platform probe and optional interrupt-backed command wait.

use alloc::{format, string::String, vec, vec::Vec};

use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
use axdriver_block::BlockDriverOps;

use crate::{AxDeviceEnum, drivers::DriverProbe};

pub struct SdMmcDriver;

type RawSdMmc = axdriver_block::sdmmc::SdMmcDriver;

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

fn gpt_partitions(dev: &RawSdMmc) -> DevResult<Vec<Partition>> {
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
    let table_bytes = count.checked_mul(entry_size).ok_or(DevError::BadState)?;
    let transfer_bytes = table_bytes
        .checked_add(block_size - 1)
        .ok_or(DevError::BadState)?
        / block_size
        * block_size;
    let table_blocks = (transfer_bytes / block_size) as u64;
    if entries_lba >= dev.num_blocks() || table_blocks > dev.num_blocks() - entries_lba {
        return Err(DevError::BadState);
    }
    let mut table = vec![0u8; transfer_bytes];
    dev.read_block(entries_lba, &mut table)?;

    let mut partitions = Vec::new();
    for (index, entry) in table[..table_bytes].chunks_exact(entry_size).enumerate() {
        if entry[..16].iter().all(|byte| *byte == 0) {
            continue;
        }
        let first = le_u64(&entry[32..40]);
        let last = le_u64(&entry[40..48]);
        if last < first || first >= dev.num_blocks() || last >= dev.num_blocks() {
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

fn partitions(dev: &RawSdMmc) -> DevResult<Vec<Partition>> {
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

fn read_bytes(dev: &RawSdMmc, offset: u64, output: &mut [u8]) -> DevResult {
    let block_size = dev.block_size();
    let mut done = 0;
    while done < output.len() {
        let absolute = offset
            .checked_add(done as u64)
            .ok_or(DevError::InvalidParam)?;
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

fn is_ext4(dev: &RawSdMmc, start_lba: u64) -> bool {
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

fn partition_number(root: &str) -> Option<usize> {
    [
        "/dev/mmcblk1p",
        "/dev/mmcblk0p",
        "/dev/mmc1p",
        "/dev/mmc0p",
        "/dev/sdmmc",
        "/dev/sda",
    ]
    .iter()
    .find_map(|prefix| root.strip_prefix(prefix)?.parse().ok())
}

fn select_partition(dev: &RawSdMmc) -> DevResult<(u64, u64, Option<Partition>)> {
    let available = partitions(dev)?;
    if let Some(root) = root_argument() {
        if matches!(
            root,
            "/dev/mmcblk1" | "/dev/mmcblk0" | "/dev/mmc1" | "/dev/mmc0" | "/dev/sdmmc" | "/dev/sda"
        ) {
            return is_ext4(dev, 0)
                .then_some((0, dev.num_blocks(), None))
                .ok_or(DevError::BadState);
        }
        let selected = if let Some(number) = partition_number(root) {
            available
                .iter()
                .find(|partition| partition.number == number)
        } else if let Some(partuuid) = root.strip_prefix("PARTUUID=") {
            available
                .iter()
                .find(|partition| partition.partuuid.eq_ignore_ascii_case(partuuid))
        } else if let Some(label) = root.strip_prefix("PARTLABEL=") {
            available.iter().find(|partition| partition.label == label)
        } else {
            return Err(DevError::Unsupported);
        }
        .ok_or(DevError::InvalidParam)?;
        return is_ext4(dev, selected.start_lba)
            .then_some((selected.start_lba, selected.blocks, Some(selected.clone())))
            .ok_or(DevError::BadState);
    }

    let candidates = available
        .iter()
        .filter(|partition| is_ext4(dev, partition.start_lba))
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [partition] => Ok((
            partition.start_lba,
            partition.blocks,
            Some((*partition).clone()),
        )),
        [] if is_ext4(dev, 0) => Ok((0, dev.num_blocks(), None)),
        _ => Err(DevError::BadState),
    }
}

/// Bounds-checked view of the SD card partition selected by `/chosen/bootargs`.
pub struct SdMmcRootDevice {
    inner: RawSdMmc,
    start_lba: u64,
    blocks: u64,
}

impl BaseDriverOps for SdMmcRootDevice {
    fn device_name(&self) -> &str {
        "sdmmc-root"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl BlockDriverOps for SdMmcRootDevice {
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

#[cfg(feature = "sdmmc-irq")]
mod interrupt {
    #[cfg(feature = "sdmmc-idmac")]
    use core::cell::UnsafeCell;
    use core::time::Duration;

    const IRQ: usize = axconfig::devices::SDMMC_IRQ as usize;

    pub(super) struct SdMmcInterrupt;

    pub(super) static INTERRUPT: SdMmcInterrupt = SdMmcInterrupt;
    static MAILBOX: axdriver_block::sdmmc::InterruptMailbox =
        axdriver_block::sdmmc::InterruptMailbox::new();
    static WAIT_QUEUE: axtask::WaitQueue = axtask::WaitQueue::new();

    fn base() -> usize {
        axhal::mem::phys_to_virt(axconfig::devices::SDMMC_PADDR.into()).into()
    }

    impl axdriver_block::sdmmc::InterruptWait for SdMmcInterrupt {
        fn prepare(&self) {
            MAILBOX.begin();
        }

        fn wait(&self, timeout: Duration) -> bool {
            !WAIT_QUEUE.wait_timeout_until(timeout, || MAILBOX.pending())
        }

        fn take_status(&self) -> u32 {
            MAILBOX.take()
        }

        fn finish(&self) {
            MAILBOX.finish();
        }

        fn rearm(&self) {
            unsafe {
                axdriver_block::sdmmc::set_interrupt_enabled(base(), true);
            }
        }

        fn current_ms(&self) -> u64 {
            axhal::time::monotonic_time_nanos() / 1_000_000
        }
    }

    fn handle() {
        let request_generation = MAILBOX.generation();
        let events = unsafe { axdriver_block::sdmmc::acknowledge_interrupt(base()) };
        MAILBOX.cache_if_current(request_generation, events);
        if request_generation != 0 && events != 0 {
            WAIT_QUEUE.notify_all(false);
            axtask::request_reschedule();
        }
    }

    pub(super) fn register() -> bool {
        if axhal::irq::register(IRQ, handle) {
            true
        } else {
            warn!("failed to register SD/MMC IRQ {IRQ}");
            false
        }
    }

    #[cfg(feature = "sdmmc-idmac")]
    const DESCRIPTOR_BYTES: usize = 4096;
    #[cfg(feature = "sdmmc-idmac")]
    const DESCRIPTOR_COUNT: usize = 16;
    #[cfg(feature = "sdmmc-idmac")]
    const BUFFER_BYTES: usize = 64 * 1024;

    #[cfg(feature = "sdmmc-idmac")]
    #[repr(C, align(4096))]
    struct DmaMemory {
        descriptors: [u8; DESCRIPTOR_BYTES],
        buffer: [u8; BUFFER_BYTES],
    }

    #[cfg(feature = "sdmmc-idmac")]
    struct SharedDmaMemory(UnsafeCell<DmaMemory>);

    #[cfg(feature = "sdmmc-idmac")]
    // SAFETY: the block driver serializes all access, and the IRQ handler never
    // touches descriptor or bounce-buffer memory.
    unsafe impl Sync for SharedDmaMemory {}

    #[cfg(feature = "sdmmc-idmac")]
    #[unsafe(link_section = ".dma_uncached")]
    static DMA_MEMORY: SharedDmaMemory = SharedDmaMemory(UnsafeCell::new(DmaMemory {
        descriptors: [0; DESCRIPTOR_BYTES],
        buffer: [0; BUFFER_BYTES],
    }));

    #[cfg(feature = "sdmmc-idmac")]
    fn virt_to_phys(address: usize) -> usize {
        axhal::mem::virt_to_phys(address.into()).as_usize()
    }

    #[cfg(feature = "sdmmc-idmac")]
    fn dma_barrier() {
        unsafe { core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags)) };
    }

    #[cfg(feature = "sdmmc-idmac")]
    pub(super) fn dma_config() -> axdriver_block::sdmmc::DmaConfig {
        let memory = DMA_MEMORY.0.get() as usize;
        // SAFETY: DMA_MEMORY is static, exclusively controller-owned,
        // 4-KiB-aligned, and placed in the platform's uncached linker section.
        unsafe {
            axdriver_block::sdmmc::DmaConfig::new(
                memory,
                DESCRIPTOR_COUNT,
                memory + DESCRIPTOR_BYTES,
                BUFFER_BYTES,
                virt_to_phys,
                dma_barrier,
            )
        }
    }
}

impl DriverProbe for SdMmcDriver {
    fn probe_global() -> Option<AxDeviceEnum> {
        #[cfg(feature = "sdmmc-irq")]
        if !interrupt::register() {
            return None;
        }

        let base = axhal::mem::phys_to_virt(axconfig::devices::SDMMC_PADDR.into()).into();
        let inner = unsafe {
            #[cfg(feature = "sdmmc-irq")]
            {
                #[cfg(feature = "sdmmc-idmac")]
                {
                    axdriver_block::sdmmc::SdMmcDriver::new_with_interrupt_and_dma(
                        base,
                        &interrupt::INTERRUPT,
                        interrupt::dma_config(),
                    )
                }
                #[cfg(not(feature = "sdmmc-idmac"))]
                {
                    axdriver_block::sdmmc::SdMmcDriver::new_with_interrupt(
                        base,
                        &interrupt::INTERRUPT,
                    )
                }
            }
            #[cfg(not(feature = "sdmmc-irq"))]
            {
                axdriver_block::sdmmc::SdMmcDriver::new(base)
            }
        };
        let (start_lba, blocks, partition) = select_partition(&inner)
            .expect("root= does not select one usable SD/MMC ext4 partition");
        info!(
            "SD/MMC root from bootargs {:?}: partition={}, label={:?}, partuuid={:?}, \
             start_lba={start_lba}, blocks={blocks}",
            root_argument(),
            partition.as_ref().map_or(0, |partition| partition.number),
            partition.as_ref().map(|partition| partition.label.as_str()),
            partition
                .as_ref()
                .map(|partition| partition.partuuid.as_str()),
        );
        Some(AxDeviceEnum::from_block(SdMmcRootDevice {
            inner,
            start_lba,
            blocks,
        }))
    }
}
