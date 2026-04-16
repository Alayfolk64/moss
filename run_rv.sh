#!/bin/sh
set -e
[ "${STARRY_TRACE_RUN:-0}" = "1" ] && set -x

if [ "${STARRY_TRACE_RUN:-0}" = "1" ]; then
  echo "[TRACE] cwd=$(pwd)"
  echo "[TRACE] uname=$(uname -a 2>/dev/null || true)"
  echo "[TRACE] PATH=$PATH"
  for t in sh bash make qemu-system-riscv64 cargo rustc rust-objcopy rust-lld xz mke2fs; do
    echo "[TRACE] tool:$t -> $(command -v "$t" 2>/dev/null || echo '<missing>')"
  done
fi

rm -rf sdcard-rv.img
xz -dk sdcard-rv.img.xz

sleep 1

make kernel-rv disk.img

sleep 1

if [ "${STARRY_TRACE_RUN:-0}" = "1" ]; then
  echo "[TRACE] file:kernel-rv -> $(ls -l kernel-rv 2>/dev/null || echo '<missing>')"
  echo "[TRACE] file:disk.img -> $(ls -l disk.img 2>/dev/null || echo '<missing>')"
  echo "[TRACE] file:sdcard-rv.img -> $(ls -l sdcard-rv.img 2>/dev/null || echo '<missing>')"
fi

if [ -f disk.img ]; then
  qemu_cmd="qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc -drive file=disk.img,if=none,format=raw,id=x1 -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1"
else
  qemu_cmd="qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc"
fi

if [ "${STARRY_TRACE_RUN:-0}" = "1" ]; then
  echo "[TRACE] qemu_cmd=$qemu_cmd"
fi

if [ "${STARRY_SKIP_QEMU:-0}" = "1" ]; then
  echo "[TRACE] STARRY_SKIP_QEMU=1, skip qemu run"
  exit 0
fi

sh -c "$qemu_cmd" 2>&1 | tee output_rv.txt
