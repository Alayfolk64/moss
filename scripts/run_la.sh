#!/bin/sh
set -e

cd ..

rm -rf kernel-la
rm -rf sdcard-la.img disk-la.img
cp ./imgs/sdcard-la.img ./

sleep 1

make kernel-la

sleep 1

qemu-system-loongarch64 -kernel kernel-la -m 1G -nographic -smp 1 \
    -drive file=sdcard-la.img,if=none,format=raw,id=x0 \
    -device virtio-blk-pci,drive=x0 -no-reboot \
    -device virtio-net-pci,netdev=net0 -netdev user,id=net0 \
    -rtc base=utc 2>&1 | tee docs/output_la.txt
