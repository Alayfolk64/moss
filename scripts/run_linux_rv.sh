#!/bin/bash

set -euo pipefail

cd "$(dirname "$0")/.."

benchmark=${1:-cagent}
case "$benchmark" in
    cagent) benchmark_script=scripts/linux_rv_benchmark.sh ;;
    buildstorm) benchmark_script=scripts/linux_rv_buildstorm.sh ;;
    profile-probe) benchmark_script=scripts/linux_rv_profile_probe.sh ;;
    profile) benchmark_script=scripts/linux_rv_profile_buildstorm.sh ;;
    *)
        echo "usage: $0 [cagent|buildstorm|profile-probe|profile]" >&2
        exit 2
        ;;
esac

kernel_cache=/home/wuxun/.cache/moss-ubuntu-rv/root-7.0.0-27
kernel="$kernel_cache/boot/Image-7.0.0-27-generic"
module_root="$kernel_cache/usr/lib/modules/7.0.0-27-generic/kernel"
artifact_root="$PWD/tmp/linux-rv"
mkdir -p "$artifact_root"
build_dir=$(mktemp -d "$artifact_root/initramfs.XXXXXX")
log_dir="$artifact_root/$benchmark-logs"

mkdir -p "$build_dir/bin" "$build_dir/modules" "$log_dir"

if [ ! -f "$kernel" ]; then
    echo "Ubuntu RISC-V kernel not found: $kernel" >&2
    exit 1
fi

debugfs -R "dump /bin/busybox $build_dir/bin/busybox" sdcard-rv.img >/dev/null
chmod +x "$build_dir/bin/busybox"
for applet in chroot insmod mkdir mount poweroff sync tee; do
    ln -s busybox "$build_dir/bin/$applet"
done
cp scripts/linux_rv_init.sh "$build_dir/init"
chmod +x "$build_dir/init"

zstd -dc "$module_root/fs/netfs/netfs.ko.zst" >"$build_dir/modules/netfs.ko"
zstd -dc "$module_root/net/9p/9pnet.ko.zst" >"$build_dir/modules/9pnet.ko"
zstd -dc "$module_root/net/9p/9pnet_virtio.ko.zst" >"$build_dir/modules/9pnet_virtio.ko"
zstd -dc "$module_root/fs/9p/9p.ko.zst" >"$build_dir/modules/9p.ko"

cp "$benchmark_script" "$log_dir/benchmark.sh"
chmod +x "$log_dir/benchmark.sh"

if [[ "$benchmark" == profile* ]]; then
    cp -a tmp/linux-rv-apt/debs "$log_dir/"
    cp scripts/linux_rv_profile.bt "$log_dir/"
fi

(
    cd "$build_dir"
    find . -print0 | cpio --null --quiet -o --format=newc | gzip -9 >initramfs.cpio.gz
)

start_ms=$(($(date +%s%N) / 1000000))
set +e
qemu-system-riscv64 \
    -machine virt \
    -kernel "$kernel" \
    -initrd "$build_dir/initramfs.cpio.gz" \
    -append "console=ttyS0 earlycon=sbi root=/dev/vda rw rootwait" \
    -m 8G \
    -smp 8 \
    -bios default \
    -nographic \
    -snapshot \
    -drive file=sdcard-rv.img,if=none,format=raw,id=rootfs \
    -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.0 \
    -fsdev local,id=hostlog,path="$log_dir",security_model=none \
    -device virtio-9p-device,fsdev=hostlog,mount_tag=hostlog \
    -no-reboot \
    2>&1 | tee "$log_dir/console.log"
qemu_status=${PIPESTATUS[0]}
set -e
end_ms=$(($(date +%s%N) / 1000000))

host_elapsed_ms=$((end_ms - start_ms))
{
    echo "qemu_status=$qemu_status"
    echo "host_elapsed_ms=$host_elapsed_ms"
} >"$log_dir/qemu-time.txt"

echo "Linux RISC-V logs: $log_dir"
echo "QEMU wall time: ${host_elapsed_ms} ms"
exit "$qemu_status"
