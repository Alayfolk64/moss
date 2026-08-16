#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source_root=${XV6_LOONGARCH_SOURCE:-"$repo_root/../xv6-loongarch-upstream"}
source_url=https://github.com/Junkher/xv6-loongarch.git
source_commit=fd994904f3230f0eaa51bda251f7e9c4bc7bf1a0
docker_image=moss-xv6-loongarch-qemu:ubuntu20.04
timestamp=$(date +%Y%m%d-%H%M%S)

mkdir -p "$repo_root/tmp"
if [[ ! -d "$source_root/.git" ]]; then
    git clone "$source_url" "$source_root"
fi
if ! git -C "$source_root" cat-file -e "$source_commit^{commit}"; then
    git -C "$source_root" fetch origin "$source_commit"
fi
actual_commit=$(git -C "$source_root" rev-parse "$source_commit^{commit}")
[[ "$actual_commit" == "$source_commit" ]] || {
    echo "xv6-loongarch commit mismatch: $actual_commit" >&2
    exit 1
}

qemu_runenv="$source_root/qemu-loongarch-runenv"
for file in \
    qemu-system-loongarch64 \
    loongarch_bios_0310.bin \
    busybox-rootfs.img \
    efi-virtio.rom; do
    [[ -f "$qemu_runenv/$file" ]] || {
        echo "missing xv6-loongarch QEMU runenv file: $qemu_runenv/$file" >&2
        exit 1
    }
done

build_root=$(mktemp -d "$repo_root/tmp/xv6-loongarch-llvm-${timestamp}.XXXXXXXX")
git -C "$source_root" archive "$source_commit" | tar -xf - -C "$build_root"
patch --batch --forward -d "$build_root" -p1 \
    <"$repo_root/scripts/patches/xv6-loongarch-llvm.patch"

build_log="$repo_root/tmp/xv6-loongarch-llvm-build-${timestamp}.log"
{
    printf '[xv6-la-build] source=%s\n' "$source_url"
    printf '[xv6-la-build] commit=%s\n' "$source_commit"
    printf '[xv6-la-build] build_root=%s\n' "$build_root"
    clang --version | head -1
    ld.lld --version
    make -C "$build_root" all \
        CC='clang --target=loongarch64-unknown-linux-gnu' \
        LD='ld.lld -m elf64loongarch' \
        AS=clang \
        OBJCOPY=llvm-objcopy \
        OBJDUMP=llvm-objdump
    llvm-readelf -h "$build_root/kernel/kernel"
    llvm-objdump -d --no-show-raw-insn \
        --start-address=0x9000000000200000 \
        --stop-address=0x9000000000200030 \
        "$build_root/kernel/kernel"
    test -x "$build_root/user/_wc"
    test -x "$build_root/user/_grep"
    sha256sum "$build_root/kernel/kernel" "$build_root/fs.img"
    echo '[xv6-la-build] PASS LLVM kernel, embedded fs, and ordinary user tools built'
} 2>&1 | tee "$build_log"

if ! docker image inspect "$docker_image" >/dev/null 2>&1; then
    docker build \
        --tag "$docker_image" \
        --file "$repo_root/scripts/docker/xv6-loongarch-qemu.Dockerfile" \
        "$repo_root/scripts/docker"
fi

boot_log="$repo_root/tmp/xv6-loongarch-llvm-qemu-${timestamp}.log"
python3 "$repo_root/scripts/run_xv6_loongarch_qemu.py" \
    --kernel "$build_root/kernel/kernel" \
    --qemu-runenv "$qemu_runenv" \
    --docker-image "$docker_image" \
    --log "$boot_log" \
    --hard-deadline 150

printf '[xv6-la] COMPLETE build_root=%s build_log=%s boot_log=%s\n' \
    "$build_root" "$build_log" "$boot_log"
