#!/bin/sh
set -e

cd ..

rm -rf sdcard-rv.img disk.img
cp ./imgs/sdcard-rv.img ./
rm -rf kernel-rv

sleep 1

make kernel-rv

sleep 1

qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default \
    -drive file=sdcard-rv.img,if=none,format=raw,id=x0 \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -no-reboot -device virtio-net-device,netdev=net \
    -netdev user,id=net -rtc base=utc 2>&1 | tee docs/output_rv.txt
