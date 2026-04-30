#!/bin/sh
set -e

# rm -rf sdcard-la.img
# rm -rf sdcard-rv.img
# cp ./imgs/sdcard-rv.img ./
# cp ./imgs/sdcard-la.img ./

# sleep 1

make all

sleep 1

if [ -f disk.img ]; then
  qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc -drive file=disk.img,if=none,format=raw,id=x1 -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1 2>&1 | tee output_rv.txt
else
  qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc 2>&1 | tee output_rv.txt
fi

sleep 1

if [ -f disk-la.img ]; then
  qemu-system-loongarch64 -kernel kernel-la -m 1G -nographic -smp 1 -drive file=sdcard-la.img,if=none,format=raw,id=x0 -device virtio-blk-pci,drive=x0 -no-reboot -device virtio-net-pci,netdev=net0 -netdev user,id=net0 -rtc base=utc -drive file=disk-la.img,if=none,format=raw,id=x1 -device virtio-blk-pci,drive=x1 2>&1 | tee output_la.txt
else
  qemu-system-loongarch64 -kernel kernel-la -m 1G -nographic -smp 1 -drive file=sdcard-la.img,if=none,format=raw,id=x0 -device virtio-blk-pci,drive=x0 -no-reboot -device virtio-net-pci,netdev=net0 -netdev user,id=net0 -rtc base=utc 2>&1 | tee output_la.txt
fi
