OUTPUT_ARCH(riscv)
ENTRY(_binary_kernel_rv_bin_start)

PHDRS {
  text PT_LOAD FLAGS(5);
}

SECTIONS {
  . = 0x80200000;
  .text : {
    _binary_kernel_rv_bin_start = .;
    *(.text)
    _binary_kernel_rv_bin_end = .;
  } :text
}
