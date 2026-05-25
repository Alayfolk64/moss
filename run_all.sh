#!/bin/sh
set -e

# rm -rf sdcard-la.img
# rm -rf sdcard-rv.img
# cp ./imgs/sdcard-rv.img ./
# cp ./imgs/sdcard-la.img ./

# sleep 1

make all

./scripts/inject_libgcc.sh riscv64 sdcard-rv.img
./scripts/inject_libgcc.sh loongarch64 sdcard-la.img

sleep 1

qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc 2>&1 | tee output_rv.txt


sleep 1

qemu-system-loongarch64 -kernel kernel-la -m 1G -nographic -smp 1 -drive file=sdcard-la.img,if=none,format=raw,id=x0 -device virtio-blk-pci,drive=x0 -no-reboot -device virtio-net-pci,netdev=net0 -netdev user,id=net0 -rtc base=utc 2>&1 | tee output_la.txt
