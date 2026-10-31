#!/bin/sh
set -e

cd "$(dirname "$0")/.."

make kernel-rv

qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -display none -smp 1 -bios default \
    -snapshot \
    -drive file=sdcard-rv.img,if=none,format=raw,id=x0 \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -no-reboot -device virtio-net-device,netdev=net \
    -netdev user,id=net -rtc base=utc \
    -chardev stdio,id=console,mux=on,signal=off,logfile=docs/output_rv.txt \
    -serial chardev:console -mon chardev=console,mode=readline
