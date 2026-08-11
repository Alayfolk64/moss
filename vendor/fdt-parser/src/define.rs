use core::{
    fmt::{Debug, Display},
    ptr::NonNull,
};

use crate::{
    error::*,
    read::{FdtReader, U32Array},
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Token {
    BeginNode,
    EndNode,
    Prop,
    Nop,
    End,
    Data,
}
impl From<u32> for Token {
    fn from(value: u32) -> Self {
        match value {
            0x1 => Token::BeginNode,
            0x2 => Token::EndNode,
            0x3 => Token::Prop,
            0x4 => Token::Nop,
            0x9 => Token::End,
            _ => Token::Data,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, align(4))]
pub struct Fdt32([u8; 4]);

impl Fdt32 {
    #[inline(always)]
    pub const fn new() -> Self {
        Self([0; 4])
    }

    #[inline(always)]
    pub fn get(self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

impl From<&[u8]> for Fdt32 {
    fn from(value: &[u8]) -> Self {
        let value = value.get(..4).unwrap();
        let mut bytes = [0u8; 4];
        for (index, byte) in bytes.iter_mut().enumerate() {
            // An FDT is a byte buffer and its base pointer is not part of this
            // API's alignment contract.  Volatile byte reads keep LLVM from
            // widening this copy on targets without unaligned access support.
            *byte = unsafe { value.as_ptr().add(index).read_volatile() };
        }
        Fdt32(bytes)
    }
}

impl Default for Fdt32 {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, align(8))]
pub struct Fdt64([u8; 8]);

impl Fdt64 {
    pub const fn new() -> Self {
        Self([0; 8])
    }

    pub fn get(&self) -> u64 {
        u64::from_be_bytes(self.0)
    }
}
impl From<&[u8]> for Fdt64 {
    fn from(value: &[u8]) -> Self {
        let value = value.get(..8).unwrap();
        let mut bytes = [0u8; 8];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = unsafe { value.as_ptr().add(index).read_volatile() };
        }
        Self(bytes)
    }
}
impl Default for Fdt64 {
    fn default() -> Self {
        Self::new()
    }
}

/// A raw `reg` property value set
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawReg<'a> {
    /// Big-endian encoded bytes making up the address portion of the property.
    /// Length will always be a multiple of 4 bytes.
    pub address: &'a [u8],
    /// Big-endian encoded bytes making up the size portion of the property.
    /// Length will always be a multiple of 4 bytes.
    pub size: &'a [u8],
}
#[derive(Debug, Clone, Copy)]
// Keep the parsed header naturally aligned even though each on-wire field is a
// byte array.  In particular, this prevents the LoongArch backend from placing
// the header at a four-byte offset inside `Result<Fdt, _>` and emitting an
// unaligned `st.d` while returning `Fdt::from_bytes`.
#[repr(C, align(8))]
pub struct FdtHeader {
    /// FDT header magic
    pub magic: Fdt32,
    /// Total size in bytes of the FDT structure
    pub totalsize: Fdt32,
    /// Offset in bytes from the start of the header to the structure block
    pub off_dt_struct: Fdt32,
    /// Offset in bytes from the start of the header to the strings block
    pub off_dt_strings: Fdt32,
    /// Offset in bytes from the start of the header to the memory reservation
    /// block
    pub off_mem_rsvmap: Fdt32,
    /// FDT version
    pub version: Fdt32,
    /// Last compatible FDT version
    pub last_comp_version: Fdt32,
    /// System boot CPU ID
    pub boot_cpuid_phys: Fdt32,
    /// Length in bytes of the strings block
    pub size_dt_strings: Fdt32,
    /// Length in bytes of the struct block
    pub size_dt_struct: Fdt32,
}

impl FdtHeader {
    #[inline(always)]
    pub(crate) fn valid_magic(&self) -> FdtResult<'static> {
        if self.magic.get() == 0xd00dfeed {
            Ok(())
        } else {
            Err(FdtError::BadMagic)
        }
    }

    #[inline(always)]
    pub(crate) fn struct_range(&self) -> core::ops::Range<usize> {
        let start = self.off_dt_struct.get() as usize;
        let end = start + self.size_dt_struct.get() as usize;

        start..end
    }

    #[inline(always)]
    pub(crate) fn strings_range(&self) -> core::ops::Range<usize> {
        let start = self.off_dt_strings.get() as usize;
        let end = start + self.size_dt_strings.get() as usize;
        start..end
    }

    #[inline(always)]
    pub fn from_bytes(bytes: &[u8]) -> FdtResult<'static, Self> {
        if bytes.len() < size_of::<FdtHeader>() {
            return Err(FdtError::Eof);
        }

        let field = |offset: usize| {
            let mut value = [0u8; 4];
            for (index, byte) in value.iter_mut().enumerate() {
                // DTBs are byte buffers. Volatile byte reads prevent LLVM's
                // LoongArch backend from widening the copy to unaligned
                // halfword/dword loads when UAL is disabled on LS2K1000LA.
                *byte = unsafe { bytes.as_ptr().add(offset + index).read_volatile() };
            }
            Fdt32(value)
        };

        Ok(Self {
            magic: field(0),
            totalsize: field(4),
            off_dt_struct: field(8),
            off_dt_strings: field(12),
            off_mem_rsvmap: field(16),
            version: field(20),
            last_comp_version: field(24),
            boot_cpuid_phys: field(28),
            size_dt_strings: field(32),
            size_dt_struct: field(36),
        })
    }

    #[inline(always)]
    pub fn from_ptr(ptr: NonNull<u8>) -> FdtResult<'static, Self> {
        let bytes = unsafe { core::slice::from_raw_parts(ptr.as_ptr(), size_of::<FdtHeader>()) };
        let header = Self::from_bytes(bytes)?;
        header.valid_magic()?;
        Ok(header)
    }

    #[inline(always)]
    pub fn total_size(&self) -> usize {
        self.totalsize.get() as _
    }
}

#[cfg(test)]
mod tests {
    use core::ptr::NonNull;

    use super::FdtHeader;

    #[test]
    fn reads_header_from_an_unaligned_byte_buffer() {
        assert_eq!(align_of::<FdtHeader>(), 8);
        assert_eq!(size_of::<FdtHeader>(), 40);

        let mut storage = [0u8; 1 + size_of::<FdtHeader>()];
        let header = &mut storage[1..];
        header[0..4].copy_from_slice(&0xd00d_feedu32.to_be_bytes());
        header[4..8].copy_from_slice(&(size_of::<FdtHeader>() as u32).to_be_bytes());

        let parsed = FdtHeader::from_bytes(header).unwrap();
        assert_eq!(parsed.total_size(), size_of::<FdtHeader>());

        let ptr = NonNull::new(header.as_mut_ptr()).unwrap();
        let parsed = FdtHeader::from_ptr(ptr).unwrap();
        assert_eq!(parsed.total_size(), size_of::<FdtHeader>());
    }
}

impl Display for FdtHeader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FdtHeader")
            .field("size", &self.totalsize.get())
            .field("version", &self.version.get())
            .field("last_comp_version", &self.last_comp_version.get())
            .finish()
    }
}

#[repr(C)]
pub(crate) struct FdtReserveEntry {
    pub address: u64,
    pub size: u64,
}
impl FdtReserveEntry {
    pub fn new(address: u64, size: u64) -> Self {
        Self { address, size }
    }
}

impl From<FdtReserveEntry> for MemoryRegion {
    fn from(value: FdtReserveEntry) -> Self {
        Self {
            address: value.address as usize as _,
            size: value.size as _,
        }
    }
}

#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub address: *mut u8,
    pub size: usize,
}

impl Debug for MemoryRegion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "MemoryRegion {{ address: {:p}, size: {:#x} }}",
            self.address, self.size
        ))
    }
}

#[derive(Clone, Copy)]
pub struct FdtReg {
    /// parent bus address
    pub address: u64,
    /// child bus address
    pub child_bus_address: u64,
    pub size: Option<usize>,
}

impl Debug for FdtReg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("<{:#x}", self.address))?;
        if self.child_bus_address != self.address {
            f.write_fmt(format_args!("({:#x})", self.child_bus_address))?;
        }
        f.write_fmt(format_args!(", "))?;
        if let Some(s) = self.size {
            f.write_fmt(format_args!("{:#x}>", s))
        } else {
            f.write_str("None>")
        }
    }
}

/// Range mapping child bus addresses to parent bus addresses
#[derive(Clone)]
pub struct FdtRange<'a> {
    data_child: &'a [u8],
    data_parent: &'a [u8],
    /// Size of range
    pub size: u64,
}

impl<'a> FdtRange<'a> {
    pub fn child_bus_address(&self) -> U32Array<'a> {
        U32Array::new(self.data_child)
    }

    pub fn parent_bus_address(&self) -> U32Array<'a> {
        U32Array::new(self.data_parent)
    }
}

impl Debug for FdtRange<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Range {{ child_bus_address: [ ")?;
        for addr in self.child_bus_address() {
            f.write_fmt(format_args!("{:#x} ", addr))?;
        }
        f.write_str("], parent_bus_address: [ ")?;
        for addr in self.parent_bus_address() {
            f.write_fmt(format_args!("{:#x} ", addr))?;
        }
        f.write_fmt(format_args!("], size: {:#x}", self.size))
    }
}

#[derive(Clone)]
pub struct FdtRangeSilce<'a> {
    address_cell: u8,
    address_cell_parent: u8,
    size_cell: u8,
    reader: FdtReader<'a>,
}

impl<'a> FdtRangeSilce<'a> {
    pub(crate) fn new(
        address_cell: u8,
        address_cell_parent: u8,
        size_cell: u8,
        reader: FdtReader<'a>,
    ) -> Self {
        Self {
            address_cell,
            address_cell_parent,
            size_cell,
            reader,
        }
    }

    pub fn iter(&self) -> FdtRangeIter<'a> {
        FdtRangeIter { s: self.clone() }
    }
}
#[derive(Clone)]
pub struct FdtRangeIter<'a> {
    s: FdtRangeSilce<'a>,
}

impl<'a> Iterator for FdtRangeIter<'a> {
    type Item = FdtRange<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let child_address_bytes = self.s.address_cell as usize * size_of::<u32>();
        let data_child = self.s.reader.take(child_address_bytes)?;

        let parent_address_bytes = self.s.address_cell_parent as usize * size_of::<u32>();
        let data_parent = self.s.reader.take(parent_address_bytes)?;

        // let child_bus_address = self.s.reader.take_by_cell_size(self.s.address_cell)?;
        // let parent_bus_address = self
        //     .s
        //     .reader
        //     .take_by_cell_size(self.s.address_cell_parent)?;
        let size = self.s.reader.take_by_cell_size(self.s.size_cell)?;
        Some(FdtRange {
            size,
            data_child,
            data_parent,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Phandle(u32);

impl From<u32> for Phandle {
    fn from(value: u32) -> Self {
        Self(value)
    }
}
impl Phandle {
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

impl Display for Phandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "<{:#x}>", self.0)
    }
}
