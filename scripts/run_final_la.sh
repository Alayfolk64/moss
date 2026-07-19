#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

cd "$ROOT_DIR"

IMAGE_DIR=${1:-final_image/images}
OUTPUT_DIR=${2:-final_image/logs}
CAGENT_IMAGE=$IMAGE_DIR/cagent-la.img
BUILDSTORM_IMAGE=$IMAGE_DIR/buildstorm-la.img

for image in "$CAGENT_IMAGE" "$BUILDSTORM_IMAGE"; do
    if [ ! -f "$image" ]; then
        echo "missing final LoongArch image: $image" >&2
        exit 1
    fi
done

mkdir -p "$OUTPUT_DIR"
make kernel-la

run_image() {
    image=$1
    output=$2

    qemu-system-loongarch64 \
        -accel tcg,thread=multi,tb-size=512 \
        -kernel kernel-la \
        -m 8G \
        -nographic \
        -smp 8 \
        -drive "file=$image,if=none,format=raw,id=x0" \
        -device virtio-blk-pci,drive=x0 \
        -no-reboot \
        -device virtio-net-pci,netdev=net0 \
        -netdev user,id=net0 \
        -rtc base=utc \
        2>&1 | tee "$output"
}

run_image "$CAGENT_IMAGE" "$OUTPUT_DIR/cagent-la.log"
run_image "$BUILDSTORM_IMAGE" "$OUTPUT_DIR/buildstorm-la.log"
