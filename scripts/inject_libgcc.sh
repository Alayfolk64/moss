#!/bin/sh
set -eu

arch=${1:?usage: inject_libgcc.sh <riscv64|loongarch64> <sdcard.img>}
img=${2:?usage: inject_libgcc.sh <riscv64|loongarch64> <sdcard.img>}

[ -f "$img" ] || exit 0
command -v debugfs >/dev/null 2>&1 || exit 0

case "$arch" in
    riscv64)
        candidates="
            /usr/riscv64-linux-gnu/lib/libgcc_s.so.1
            /opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libgcc_s.so.1
            /opt/riscv64-lp64d--musl--bleeding-edge-2024.02-1/riscv64-buildroot-linux-musl/lib/libgcc_s.so.1
            /opt/riscv64-lp64d--musl--bleeding-edge-2024.02-1/riscv64-buildroot-linux-musl/sysroot/lib/libgcc_s.so.1
        "
        ;;
    loongarch64)
        candidates="
            /opt/gcc-13.2.0-loongarch64-linux-gnu/loongarch64-linux-gnu/lib64/libgcc_s.so.1
            /opt/toolchain-loongarch64-linux-gnu-gcc8-host-x86_64-2022-07-18/sysroot/usr/lib64/libgcc_s.so.1
            /opt/loongarch64-linux-musl-cross/loongarch64-linux-musl/lib/libgcc_s.so.1
        "
        ;;
    *)
        exit 0
        ;;
esac

if debugfs -R "stat /glibc/lib/libgcc_s.so.1" "$img" 2>/dev/null | grep -q '^Inode:'; then
    exit 0
fi

src=
for candidate in $candidates; do
    if [ -f "$candidate" ]; then
        src=$candidate
        break
    fi
done

[ -n "$src" ] || exit 0

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
{
    echo "write $src /glibc/lib/libgcc_s.so.1"
} >"$tmp"

debugfs -w -f "$tmp" "$img" >/dev/null
