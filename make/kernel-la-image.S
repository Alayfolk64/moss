#ifndef KERNEL_BINARY
#error KERNEL_BINARY must name the raw LoongArch64 kernel image
#endif

.section .text,"ax",@progbits
.global _binary_kernel_la_bin_start
.global _binary_kernel_la_bin_end
.balign 16
_binary_kernel_la_bin_start:
.incbin KERNEL_BINARY
_binary_kernel_la_bin_end:
