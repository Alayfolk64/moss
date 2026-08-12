use core::mem::size_of;

pub(crate) const INT_TI: u32 = 1 << 0;
pub(crate) const INT_RI: u32 = 1 << 1;
pub(crate) const INT_FBE: u32 = 1 << 2;
pub(crate) const INT_DU: u32 = 1 << 4;
pub(crate) const INT_CES: u32 = 1 << 5;
pub(crate) const INT_NI: u32 = 1 << 8;
pub(crate) const INT_AI: u32 = 1 << 9;
pub(crate) const INT_ERROR: u32 = INT_FBE | INT_DU | INT_CES | INT_AI;
pub(crate) const INT_CLEAR: u32 = INT_TI | INT_RI | INT_ERROR | INT_NI;
pub(crate) const INT_ENABLE: u32 = INT_NI | INT_TI | INT_RI | INT_ERROR;

const DESC_OWN: u32 = 1 << 31;
const DESC_CH: u32 = 1 << 4;
const DESC_FS: u32 = 1 << 3;
const DESC_LD: u32 = 1 << 2;
const DESC_DIC: u32 = 1 << 1;
const DESC_MAX_BYTES: usize = 4096;

/// Platform-owned coherent memory and address hooks used by DWMMC IDMAC.
#[derive(Clone, Copy, Debug)]
pub struct DmaConfig {
    pub(crate) descriptors: usize,
    pub(crate) descriptor_count: usize,
    pub(crate) buffer: usize,
    pub(crate) buffer_len: usize,
    pub(crate) virt_to_phys: fn(usize) -> usize,
    pub(crate) barrier: fn(),
}

impl DmaConfig {
    /// Describes coherent storage reserved exclusively for this controller.
    ///
    /// # Safety
    ///
    /// Both regions must remain valid and exclusively owned for the driver
    /// lifetime. Descriptors must be 16-byte aligned and the data buffer must
    /// be 4-KiB aligned and mapped uncached.
    pub const unsafe fn new(
        descriptors: usize,
        descriptor_count: usize,
        buffer: usize,
        buffer_len: usize,
        virt_to_phys: fn(usize) -> usize,
        barrier: fn(),
    ) -> Self {
        Self {
            descriptors,
            descriptor_count,
            buffer,
            buffer_len,
            virt_to_phys,
            barrier,
        }
    }

    pub(crate) fn max_transfer_len(self) -> usize {
        self.buffer_len
            .min(self.descriptor_count.saturating_mul(DESC_MAX_BYTES))
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Descriptor {
    control: u32,
    size: u32,
    buffer: u32,
    next: u32,
}

pub(crate) fn prepare_descriptors(
    descriptors: &mut [Descriptor],
    table_phys: usize,
    buffer_phys: usize,
    len: usize,
) -> Option<usize> {
    if len == 0
        || table_phys > u32::MAX as usize
        || buffer_phys > u32::MAX as usize
        || buffer_phys.checked_add(len)? > u32::MAX as usize + 1
    {
        return None;
    }

    let mut offset = 0;
    let mut used = 0;
    while offset < len {
        let dma = buffer_phys.checked_add(offset)?;
        let boundary = DESC_MAX_BYTES - dma % DESC_MAX_BYTES;
        let chunk = (len - offset).min(DESC_MAX_BYTES).min(boundary);
        if used == descriptors.len() {
            return None;
        }
        let last = offset + chunk == len;
        let next = if last {
            0
        } else {
            table_phys.checked_add((used + 1) * size_of::<Descriptor>())?
        };
        if next > u32::MAX as usize {
            return None;
        }

        let mut control = DESC_OWN;
        if used == 0 {
            control |= DESC_FS;
        }
        if last {
            control |= DESC_LD;
        } else {
            control |= DESC_CH | DESC_DIC;
        }
        descriptors[used] = Descriptor {
            control,
            size: chunk as u32,
            buffer: dma as u32,
            next: next as u32,
        };
        used += 1;
        offset += chunk;
    }
    descriptors[used..].fill(Descriptor::default());
    Some(used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_descriptor_has_first_and_last_bits() {
        let mut descriptors = [Descriptor::default(); 2];
        assert_eq!(
            prepare_descriptors(&mut descriptors, 0x1000, 0x4000, 512),
            Some(1)
        );
        assert_eq!(
            descriptors[0],
            Descriptor {
                control: DESC_OWN | DESC_FS | DESC_LD,
                size: 512,
                buffer: 0x4000,
                next: 0,
            }
        );
    }

    #[test]
    fn descriptor_never_crosses_a_four_kib_boundary() {
        let mut descriptors = [Descriptor::default(); 2];
        assert_eq!(
            prepare_descriptors(&mut descriptors, 0x1000, 0x4f00, 512),
            Some(2)
        );
        assert_eq!(descriptors[0].size, 256);
        assert_eq!(descriptors[1].size, 256);
        assert_eq!(descriptors[1].buffer, 0x5000);
    }

    #[test]
    fn sixteen_descriptors_cover_the_full_bounce_buffer() {
        let mut descriptors = [Descriptor::default(); 16];
        assert_eq!(
            prepare_descriptors(&mut descriptors, 0x1000, 0x20_0000, 64 * 1024),
            Some(16)
        );
        assert_eq!(
            descriptors[0].control,
            DESC_OWN | DESC_FS | DESC_CH | DESC_DIC
        );
        assert_eq!(descriptors[15].control, DESC_OWN | DESC_LD);
        assert_eq!(descriptors[15].next, 0);
    }
}
