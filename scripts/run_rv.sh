#!/bin/sh
set -e

cd "$(dirname "$0")/.."

make kernel-rv

qemu-system-riscv64 -machine virt -kernel kernel-rv -m 8G -nographic -smp 8 -bios default \
    -snapshot \
    -drive file=sdcard-rv.img,if=none,format=raw,id=x0 \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -no-reboot -device virtio-net-device,netdev=net \
    -netdev user,id=net -rtc base=utc 2>&1 | tee docs/output_rv.txt
