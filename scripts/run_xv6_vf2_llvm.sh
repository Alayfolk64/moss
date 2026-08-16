#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source_root=${XV6_MARS_SOURCE:-"$repo_root/../xv6-mars-upstream"}
source_url=https://github.com/GerfautGE/xv6-mars.git
source_commit=3ee0574c570fb750a6327b7c2c07a7f05634fb49
docker_image=moss-xv6-riscv-musl:alpine3.22
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
    echo "xv6-mars commit mismatch: $actual_commit" >&2
    exit 1
}

build_root=$(mktemp -d "$repo_root/tmp/xv6-mars-vf2-${timestamp}.XXXXXXXX")
git -C "$source_root" archive "$source_commit" | tar -xf - -C "$build_root"
patch --batch --forward -d "$build_root" -p1 \
    <"$repo_root/scripts/patches/xv6-mars-llvm-vf2.patch"
cp "$repo_root/scripts/patches/xv6-mars-vf2.config.mk" \
    "$build_root/config.mk"

if ! docker image inspect "$docker_image" >/dev/null 2>&1; then
    docker build \
        --tag "$docker_image" \
        --file "$repo_root/scripts/docker/xv6-riscv-musl.Dockerfile" \
        "$repo_root/scripts/docker"
fi

build_log="$repo_root/tmp/xv6-mars-vf2-llvm-musl-build-${timestamp}.log"
{
    printf '[xv6-vf2-build] source=%s\n' "$source_url"
    printf '[xv6-vf2-build] commit=%s\n' "$source_commit"
    printf '[xv6-vf2-build] build_root=%s\n' "$build_root"
    docker run --rm \
        --network none \
        --volume "$build_root:/src" \
        --workdir /src \
        "$docker_image" \
        sh -ec '
            ldd --version 2>&1 | head -n 1
            clang --version | head -n 1
            ld.lld --version
            make clean \
                TOOLPREFIX=llvm- \
                CC="clang --target=riscv64-unknown-elf" \
                LD=ld.lld \
                OBJCOPY=llvm-objcopy \
                OBJDUMP=llvm-objdump \
                STRIP=llvm-strip
            make kernel/kernel fs.img \
                RUST_UPROGS= \
                TOOLPREFIX=llvm- \
                CC="clang --target=riscv64-unknown-elf" \
                AS="clang --target=riscv64-unknown-elf" \
                LD=ld.lld \
                OBJCOPY=llvm-objcopy \
                OBJDUMP=llvm-objdump \
                STRIP=llvm-strip
        '
    llvm-readelf -h -l "$build_root/kernel/kernel"
    for tool in cat echo grep ls mkdir reboot rm sh wc; do
        test -x "$build_root/user/_$tool"
    done
    stat -c '%n %s bytes' "$build_root/kernel/kernel" "$build_root/fs.img"
    sha256sum "$build_root/kernel/kernel" "$build_root/fs.img"
    echo '[xv6-vf2-build] PASS Alpine/musl LLVM kernel, fs, and ordinary tools built'
} 2>&1 | tee "$build_log"

printf '[xv6-vf2-build] COMPLETE build_root=%s build_log=%s\n' \
    "$build_root" "$build_log"
