#!/bin/sh
set -e

if command -v make >/dev/null 2>&1 && command -v docker >/dev/null 2>&1; then
  make --no-print-directory repair-sdcard-rv
fi

qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc 2>&1 | tee output_rv.txt
