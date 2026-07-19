#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

cd "$ROOT_DIR"

IMAGE_DIR=${1:-final_image/images}
OUTPUT_DIR=${2:-final_image/logs}
CAGENT_IMAGE=$IMAGE_DIR/cagent-rv.img
BUILDSTORM_IMAGE=$IMAGE_DIR/buildstorm-rv.img

for image in "$CAGENT_IMAGE" "$BUILDSTORM_IMAGE"; do
    if [ ! -f "$image" ]; then
        echo "missing final RISC-V image: $image" >&2
        exit 1
    fi
done

mkdir -p "$OUTPUT_DIR"
make kernel-rv

run_image() {
    image=$1
    output=$2

    qemu-system-riscv64 \
        -machine virt \
        -kernel kernel-rv \
        -m 1G \
        -nographic \
        -smp 1 \
        -bios default \
        -drive "file=$image,if=none,format=raw,id=x0" \
        -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
        -no-reboot \
        -device virtio-net-device,netdev=net \
        -netdev user,id=net \
        -rtc base=utc \
        2>&1 | tee "$output"
}

run_image "$CAGENT_IMAGE" "$OUTPUT_DIR/cagent-rv.log"
run_image "$BUILDSTORM_IMAGE" "$OUTPUT_DIR/buildstorm-rv.log"
