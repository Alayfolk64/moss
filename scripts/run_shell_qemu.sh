#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ==================== User configuration ====================
# Change these two paths when switching to another rootfs image.
RV_IMAGE="${REPO_ROOT}/images/on-site-final-2025/alpine-linux-riscv64-ext4fs.img"
LA_IMAGE="${REPO_ROOT}/images/on-site-final-2025/alpine-linux-loongarch64-ext4fs.img"

MEMORY="8G"
RV_CPUS="8"
LA_CPUS="12"

# 1: discard rootfs changes when QEMU exits; 0: write changes to the image.
USE_SNAPSHOT="1"
# ============================================================

usage() {
    echo "Usage: $0 <rv|la>"
}

if [[ $# -ne 1 ]]; then
    usage
    exit 2
fi

# The top-level Makefile derives A from PWD, so enter the repository for real.
# `make -C` alone does not reliably update the imported PWD value.
cd "${REPO_ROOT}"

snapshot_args=()
if [[ "${USE_SNAPSHOT}" == "1" ]]; then
    snapshot_args=(-snapshot)
elif [[ "${USE_SNAPSHOT}" != "0" ]]; then
    echo "USE_SNAPSHOT must be 0 or 1" >&2
    exit 2
fi

case "$1" in
    rv|riscv64)
        if [[ ! -f "${RV_IMAGE}" ]]; then
            echo "RISC-V rootfs image not found: ${RV_IMAGE}" >&2
            exit 1
        fi
        command -v qemu-system-riscv64 >/dev/null

        make -C "${REPO_ROOT}" \
            ARCH=riscv64 SHELL_INIT=y MEM="${MEMORY}" SMP="${RV_CPUS}" \
            kernel-rv

        exec qemu-system-riscv64 \
            -machine virt \
            -bios default \
            -kernel "${REPO_ROOT}/kernel-rv" \
            -m "${MEMORY}" \
            -smp "${RV_CPUS}" \
            -nographic \
            "${snapshot_args[@]}" \
            -drive "file=${RV_IMAGE},if=none,format=raw,id=x0" \
            -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
            -device virtio-net-device,netdev=net \
            -netdev user,id=net \
            -rtc base=utc \
            -no-reboot
        ;;

    la|loongarch64)
        if [[ ! -f "${LA_IMAGE}" ]]; then
            echo "LoongArch rootfs image not found: ${LA_IMAGE}" >&2
            exit 1
        fi
        command -v qemu-system-loongarch64 >/dev/null

        make -C "${REPO_ROOT}" \
            ARCH=loongarch64 BUS=pci SHELL_INIT=y \
            MEM="${MEMORY}" SMP="${LA_CPUS}" kernel-la

        exec qemu-system-loongarch64 \
            -machine virt \
            -accel tcg,thread=multi,tb-size=512 \
            -kernel "${REPO_ROOT}/kernel-la" \
            -m "${MEMORY}" \
            -smp "${LA_CPUS}" \
            -nographic \
            "${snapshot_args[@]}" \
            -drive "file=${LA_IMAGE},if=none,format=raw,id=x0" \
            -device virtio-blk-pci,drive=x0 \
            -device virtio-net-pci,netdev=net0 \
            -netdev user,id=net0 \
            -rtc base=utc \
            -no-reboot
        ;;

    *)
        usage
        exit 2
        ;;
esac
