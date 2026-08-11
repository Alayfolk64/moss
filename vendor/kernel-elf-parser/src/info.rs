//! ELF information parsed from the ELF file

use alloc::vec::Vec;
use core::ops::Range;

use xmas_elf::{
    header::Class,
    program::{ProgramHeader32, ProgramHeader64},
};

use crate::auxv::{AuxEntry, AuxType};

pub struct ELFHeadersBuilder<'a>(ELFHeaders<'a>);
impl<'a> ELFHeadersBuilder<'a> {
    pub fn new(input: &'a [u8]) -> Result<Self, &'static str> {
        Ok(Self(ELFHeaders {
            header: xmas_elf::header::parse_header(input)?,
            ph: Vec::new(),
        }))
    }

    pub fn ph_range(&self) -> Range<u64> {
        let start = self.0.header.pt2.ph_offset();
        let size = self.0.header.pt2.ph_entry_size() as u64 * self.0.header.pt2.ph_count() as u64;
        start..start + size
    }

    pub fn build(mut self, ph: &[u8]) -> Result<ELFHeaders<'a>, &'static str> {
        self.0.ph = ph
            .chunks_exact(self.0.header.pt2.ph_entry_size() as usize)
            .map(|chunk| match self.0.header.pt1.class() {
                Class::ThirtyTwo => {
                    let ph: &ProgramHeader32 = zero::read(chunk);
                    ProgramHeader64 {
                        type_: ph.type_,
                        offset: ph.offset as _,
                        virtual_addr: ph.virtual_addr as _,
                        physical_addr: ph.physical_addr as _,
                        file_size: ph.file_size as _,
                        mem_size: ph.mem_size as _,
                        flags: ph.flags,
                        align: ph.align as _,
                    }
                }
                Class::SixtyFour => *zero::read(chunk),
                Class::None | Class::Other(_) => unreachable!(),
            })
            .collect();
        Ok(self.0)
    }
}

pub struct ELFHeaders<'a> {
    pub header: xmas_elf::header::Header<'a>,
    pub ph: Vec<ProgramHeader64>,
}

/// A wrapper for the ELF file data with some useful methods.
pub struct ELFParser<'a> {
    headers: &'a ELFHeaders<'a>,
    /// Base address of the ELF file loaded into the memory.
    base: usize,
}

impl<'a> ELFParser<'a> {
    /// Create a new `ELFInfo` instance.
    pub fn new(headers: &'a ELFHeaders<'a>, bias: usize) -> Result<Self, &'static str> {
        let base = if headers.header.pt2.type_().as_type() == xmas_elf::header::Type::SharedObject {
            bias
        } else {
            0
        };
        Ok(Self { headers, base })
    }

    /// The entry point of the ELF file.
    pub fn entry(&self) -> usize {
        // TODO: base_load_address_offset?
        self.headers.header.pt2.entry_point() as usize + self.base
    }

    /// The number of program headers in the ELF file.
    pub fn phnum(&self) -> usize {
        self.headers.header.pt2.ph_count() as usize
    }

    /// The size of the program header table entry in the ELF file.
    pub fn phent(&self) -> usize {
        self.headers.header.pt2.ph_entry_size() as usize
    }

    /// The in-memory address of the program header table.
    ///
    /// Linux exposes this address through `AT_PHDR`, so the complete table must
    /// be covered by a loadable segment.  Malformed or unsupported layouts are
    /// rejected instead of panicking the kernel while handling `execve`.
    pub fn phdr(&self) -> Result<usize, &'static str> {
        let ph_start = self.headers.header.pt2.ph_offset();
        let ph_size = u64::from(self.headers.header.pt2.ph_entry_size())
            .checked_mul(u64::from(self.headers.header.pt2.ph_count()))
            .ok_or("program header table size overflows")?;
        let ph_end = ph_start
            .checked_add(ph_size)
            .ok_or("program header table range overflows")?;

        let segment = self
            .headers
            .ph
            .iter()
            .filter(|header| header.get_type() == Ok(xmas_elf::program::Type::Load))
            .find(|header| {
                header
                    .offset
                    .checked_add(header.file_size)
                    .is_some_and(|segment_end| header.offset <= ph_start && ph_end <= segment_end)
            })
            .ok_or("program header table is not covered by a loadable segment")?;

        let offset_in_segment = ph_start
            .checked_sub(segment.offset)
            .ok_or("invalid program header table offset")?;
        let address = segment
            .virtual_addr
            .checked_add(offset_in_segment)
            .and_then(|address| address.checked_add(self.base as u64))
            .ok_or("program header table address overflows")?;
        usize::try_from(address).map_err(|_| "program header table address does not fit usize")
    }

    /// The base address of the ELF file loaded into the memory.
    pub fn base(&self) -> usize {
        self.base
    }

    pub fn headers(&self) -> &'a ELFHeaders<'a> {
        self.headers
    }

    /// Part of auxiliary vectors from the ELF file.
    ///
    /// # Arguments
    ///
    /// * `pagesz` - The page size of the system
    /// * `ldso_base` - The base address of the dynamic linker (if exists)
    ///
    /// Details about auxiliary vectors are described in <https://articles.manugarg.com/aboutelfauxiliaryvectors.html>
    pub fn aux_vector(
        &self,
        pagesz: usize,
        ldso_base: Option<usize>,
    ) -> Result<impl Iterator<Item = AuxEntry>, &'static str> {
        let phdr = self.phdr()?;
        Ok([
            (AuxType::PHDR, phdr),
            (AuxType::PHENT, self.phent()),
            (AuxType::PHNUM, self.phnum()),
            (AuxType::PAGESZ, pagesz),
            (AuxType::ENTRY, self.entry()),
        ]
        .into_iter()
        .chain(ldso_base.into_iter().map(|base| (AuxType::BASE, base)))
        .map(|(at, val)| AuxEntry::new(at, val)))
    }
}
