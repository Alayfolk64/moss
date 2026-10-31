#!/bin/sh
set -e

cd "$(dirname "$0")/.."

make kernel-la

qemu-system-loongarch64 -accel tcg,thread=multi,tb-size=512 \
    -kernel kernel-la -m 8G -display none -smp 8 \
    -snapshot \
    -drive file=sdcard-la.img,if=none,format=raw,id=x0 \
    -device virtio-blk-pci,drive=x0 -no-reboot \
    -device virtio-net-pci,netdev=net0 -netdev user,id=net0 \
    -rtc base=utc \
    -chardev stdio,id=console,mux=on,signal=off,logfile=docs/output_la.txt \
    -serial chardev:console -mon chardev=console,mode=readline
