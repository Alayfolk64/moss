#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "$repo_root/tmp"
export TMPDIR="$repo_root/tmp"

usage() {
    echo "usage: $0 {rv|la|all}" >&2
}

[[ $# -eq 1 ]] || { usage; exit 2; }
requested_arch=$1
case "$requested_arch" in rv|la|all) ;; *) usage; exit 2 ;; esac

for tool in cp date docker e2fsck find grep make mktemp mountpoint python3 \
    resize2fs rmdir sha256sum sudo sync truncate umount wc; do
    command -v "$tool" >/dev/null || {
        echo "missing host tool: $tool" >&2
        exit 1
    }
done
sudo -n true

mount_dir=
cleanup() {
    if [[ -n "$mount_dir" ]] && mountpoint -q "$mount_dir"; then
        sudo umount "$mount_dir" || true
    fi
    if [[ -n "$mount_dir" && -d "$mount_dir" ]]; then
        rmdir "$mount_dir" || true
    fi
}
trap cleanup EXIT

repair_ext4() {
    local image=$1 rc
    set +e
    sudo e2fsck -fy "$image"
    rc=$?
    set -e
    [[ "$rc" -le 1 ]]
}

prepare_packages() {
    local release=$1 apk_arch=$2 cache=$3
    local version="gdb-native-v1-$release-$apk_arch"
    mkdir -p "$cache"
    if [[ -f "$cache/.ready" ]] && [[ $(<"$cache/.ready") == "$version" ]]; then
        return
    fi
    find "$cache" -maxdepth 1 -type f -name '*.apk' -delete
    docker run --rm -v "$cache:/packages" "alpine:$release" sh -ec \
        "printf '%s\\n' http://mirrors.aliyun.com/alpine/v$release/main http://mirrors.aliyun.com/alpine/v$release/community > /tmp/repositories; apk --allow-untrusted --no-cache --arch '$apk_arch' --repositories-file /tmp/repositories fetch --recursive --output /packages gdb gcc musl-dev binutils file"
    printf '%s\n' "$version" >"$cache/.ready"
}

run_arch() {
    local arch=$1 release apk_arch base_image kernel make_target qemu_binary
    case "$arch" in
        rv)
            release=3.22
            apk_arch=riscv64
            base_image="$repo_root/images/on-site-final-2025/alpine-linux-riscv64-ext4fs.img"
            kernel="$repo_root/kernel-rv"
            make_target='kernel-rv'
            qemu_binary=qemu-system-riscv64
            ;;
        la)
            release=3.21
            apk_arch=loongarch64
            base_image="$repo_root/images/on-site-final-2025/alpine-linux-loongarch64-ext4fs.img"
            kernel="$repo_root/kernel-la"
            make_target='kernel-la'
            qemu_binary=qemu-system-loongarch64
            ;;
    esac
    command -v "$qemu_binary" >/dev/null
    [[ -f "$base_image" ]]

    local package_cache="$repo_root/tmp/qemu-gdb-packages-$release-$apk_arch"
    prepare_packages "$release" "$apk_arch" "$package_cache"
    for package in gdb gcc musl-dev binutils file; do
        find "$package_cache" -maxdepth 1 -type f -name "$package-[0-9]*.apk" \
            -print -quit | grep -q . || {
            echo "required APK missing for $apk_arch: $package" >&2
            return 1
        }
    done
    local package_count
    package_count=$(find "$package_cache" -maxdepth 1 -type f -name '*.apk' | wc -l)
    [[ "$package_count" -ge 15 ]]

    local working_image="$repo_root/tmp/qemu-$arch-gdb-native.img"
    cp -f --reflink=auto "$base_image" "$working_image"
    truncate -s 2G "$working_image"
    repair_ext4 "$working_image"
    sudo resize2fs "$working_image"
    mount_dir=$(mktemp -d "$repo_root/tmp/qemu-$arch-gdb-mount.XXXXXX")
    sudo mount -o loop "$working_image" "$mount_dir"
    sudo rm -rf -- "$mount_dir/var/cache/moss-gdb-native"
    sudo install -d -m 0755 "$mount_dir/var/cache/moss-gdb-native"
    sudo cp -a "$package_cache"/*.apk "$mount_dir/var/cache/moss-gdb-native/"
    sudo install -m 0644 "$repo_root/tests/gdb-native-demo.c" \
        "$repo_root/tests/gdb-native-smoke.gdb" \
        "$mount_dir/var/cache/moss-gdb-native/"
    sync
    sudo umount "$mount_dir"
    rmdir "$mount_dir"
    mount_dir=
    repair_ext4 "$working_image"

    if [[ ${QEMU_GDB_SKIP_BUILD:-0} != 1 ]]; then
        if [[ "$arch" == rv ]]; then
            make -B -C "$repo_root" ARCH=riscv64 BUS=mmio SHELL_INIT=y \
                MEM=2G SMP=2 CPU_CAPACITY=2 "$make_target"
        else
            make -B -C "$repo_root" ARCH=loongarch64 BUS=pci SHELL_INIT=y \
                MEM=2G SMP=2 CPU_CAPACITY=2 "$make_target"
        fi
    fi
    [[ -f "$kernel" ]]

    local timestamp log hard_deadline=900
    timestamp=$(date +%Y%m%d-%H%M%S)
    log="$repo_root/tmp/qemu-$arch-gdb-native-$timestamp.log"
    {
        printf '[qemu-gdb] arch=%s\n' "$apk_arch"
        printf '[qemu-gdb] alpine=%s\n' "$release"
        printf '[qemu-gdb] qemu=%s\n' "$($qemu_binary --version | head -n 1)"
        printf '[qemu-gdb] commit=%s\n' "$(git -C "$repo_root" rev-parse HEAD)"
        printf '[qemu-gdb] base_image_sha256=%s\n' "$(sha256sum "$base_image" | awk '{print $1}')"
        printf '[qemu-gdb] kernel_sha256=%s\n' "$(sha256sum "$kernel" | awk '{print $1}')"
        printf '[qemu-gdb] package_count=%s\n' "$package_count"
    } | tee "$log"

    python3 "$repo_root/scripts/run_qemu_gdb_native.py" \
        --arch "$arch" --kernel "$kernel" --image "$working_image" \
        --log "$log" --hard-deadline "$hard_deadline" --install-only
    repair_ext4 "$working_image"
    python3 "$repo_root/scripts/run_qemu_gdb_native.py" \
        --arch "$arch" --kernel "$kernel" --image "$working_image" \
        --log "$log" --hard-deadline "$hard_deadline"

    local fsck_output fsck_rc
    set +e
    fsck_output=$(sudo e2fsck -fn "$working_image" 2>&1)
    fsck_rc=$?
    set -e
    printf '%s\n' "$fsck_output" | tee -a "$log"
    if [[ "$fsck_rc" -ne 0 ]] || grep -Eq \
        'UNEXPECTED INCONSISTENCY|FILE SYSTEM WAS MODIFIED|still has errors' \
        <<<"$fsck_output"; then
        echo "[qemu-gdb] FAIL read-only fsck rc=$fsck_rc" | tee -a "$log"
        return 1
    fi
    echo "[qemu-gdb] PASS working image e2fsck read-only check" | tee -a "$log"
    echo "[qemu-gdb] log=$log"
}

case "$requested_arch" in
    rv) run_arch rv ;;
    la) run_arch la ;;
    all) run_arch rv; run_arch la ;;
esac
