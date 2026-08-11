use crate::{FdtRangeSilce, Phandle};

#[derive(Clone, Default)]
// `Option<u8>` has byte alignment.  The pinned LLVM LoongArch backend merges
// adjacent default-initialization bytes and can emit an unaligned halfword
// store when Rust's default field reordering places them at an odd offset.
// Keep the byte-sized cell metadata in declaration order at even offsets and
// give every array element natural machine alignment.  This matters on LA264,
// which reports CPUCFG1.UAL=0 and therefore traps such widened accesses.
#[repr(C, align(8))]
pub(crate) struct MetaData<'a> {
    pub address_cells: Option<u8>,
    pub size_cells: Option<u8>,
    pub clock_cells: Option<u8>,
    pub interrupt_cells: Option<u8>,
    pub gpio_cells: Option<u8>,
    pub dma_cells: Option<u8>,
    pub cooling_cells: Option<u8>,
    // Make the alignment gap before `range` initialized data.  Otherwise the
    // backend copies the last `Option<u8>` together with undefined padding by
    // an odd-address `st.h` in large array initializers.
    _cell_padding: [u8; 2],
    pub range: Option<FdtRangeSilce<'a>>,
    pub interrupt_parent: Option<Phandle>,
}
