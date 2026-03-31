OUTPUT_ARCH(loongarch64)
ENTRY(__kernel_la_entry)

PHDRS {
  text PT_LOAD FLAGS(5);
}

SECTIONS {
  . = 0x00200000;
  .text : {
    _binary_kernel_la_bin_start = .;
    __kernel_la_entry = . + 0x40;
    *(.text)
    _binary_kernel_la_bin_end = .;
  } :text
}
