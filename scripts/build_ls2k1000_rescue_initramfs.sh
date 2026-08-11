#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: sudo $0 <mounted-loongarch-linux-root> <output.cpio.gz>" >&2
    exit 2
fi

if [[ ${EUID} -ne 0 ]]; then
    echo "error: run through sudo so the initramfs has root ownership" >&2
    exit 2
fi

source_root=$(readlink -f -- "$1")
output_image=$(readlink -m -- "$2")
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

if [[ ! -x "$source_root/bin/busybox" || ! -x "$source_root/sbin/e2fsck" ]]; then
    echo "error: source root does not contain the required LoongArch rescue tools" >&2
    exit 1
fi

rescue_stage=$(mktemp -d /tmp/moss-ls2k1000-rescue.XXXXXX)
cleanup() {
    case "$rescue_stage" in
        /tmp/moss-ls2k1000-rescue.*)
            find "$rescue_stage" -depth -delete
            ;;
        *)
            echo "refusing to clean unexpected path: $rescue_stage" >&2
            ;;
    esac
}
trap cleanup EXIT

install -d -m 0755 \
    "$rescue_stage/bin" \
    "$rescue_stage/sbin" \
    "$rescue_stage/lib64" \
    "$rescue_stage/usr" \
    "$rescue_stage/etc" \
    "$rescue_stage/dev" \
    "$rescue_stage/proc" \
    "$rescue_stage/sys" \
    "$rescue_stage/run" \
    "$rescue_stage/tmp" \
    "$rescue_stage/mnt"
chmod 1777 "$rescue_stage/tmp"

rescue_binaries=(
    /bin/bash
    /bin/busybox
    /bin/coreutils
    /bin/mount
    /bin/umount
    /sbin/blkid
    /sbin/e2fsck
    /sbin/fdisk
    /sbin/ip
    /sbin/mke2fs
    /sbin/partprobe
    /sbin/resize2fs
    /sbin/sfdisk
    /sbin/tune2fs
)

dependency_queue=()
for binary_path in "${rescue_binaries[@]}"; do
    if [[ ! -e "$source_root$binary_path" ]]; then
        echo "error: missing rescue binary: $binary_path" >&2
        exit 1
    fi
    install -D -m 0755 "$source_root$binary_path" "$rescue_stage$binary_path"
    dependency_queue+=("$source_root$binary_path")
done

# All binaries in the board image request this interpreter.  Store the
# resolved loader at the exact ABI path without reproducing the rootfs's
# /lib -> /usr/lib symlink layout.
loader_source=$(readlink -f -- "$source_root/lib64/ld.so.1")
install -m 0755 "$loader_source" "$rescue_stage/lib64/ld.so.1"
ln -s lib64 "$rescue_stage/lib"
ln -s ../lib64 "$rescue_stage/usr/lib"

declare -A copied_libraries=()
queue_index=0
while (( queue_index < ${#dependency_queue[@]} )); do
    elf_file=${dependency_queue[$queue_index]}
    ((queue_index += 1))

    while IFS= read -r library_name; do
        [[ -n "$library_name" ]] || continue
        if [[ -n ${copied_libraries[$library_name]+present} ]]; then
            continue
        fi

        library_source=""
        for library_dir in lib lib64 usr/lib usr/lib64; do
            library_candidate="$source_root/$library_dir/$library_name"
            if [[ -e "$library_candidate" ]]; then
                library_source=$(readlink -f -- "$library_candidate")
                break
            fi
        done
        if [[ -z "$library_source" ]]; then
            echo "error: cannot resolve $library_name required by $elf_file" >&2
            exit 1
        fi

        install -m 0755 "$library_source" "$rescue_stage/lib64/$library_name"
        copied_libraries[$library_name]=1
        dependency_queue+=("$library_source")
    done < <(
        readelf -d "$elf_file" 2>/dev/null |
            sed -n 's/.*Shared library: \[\([^]]*\)\].*/\1/p'
    )
done

if [[ -f "$source_root/etc/e2fsck.conf" ]]; then
    install -m 0644 "$source_root/etc/e2fsck.conf" "$rescue_stage/etc/e2fsck.conf"
fi
if [[ -f "$source_root/etc/mke2fs.conf" ]]; then
    install -m 0644 "$source_root/etc/mke2fs.conf" "$rescue_stage/etc/mke2fs.conf"
fi

ln -s busybox "$rescue_stage/bin/sh"
ln -s e2fsck "$rescue_stage/sbin/fsck.ext4"
ln -s mke2fs "$rescue_stage/sbin/mkfs.ext4"

mknod -m 0600 "$rescue_stage/dev/console" c 5 1
mknod -m 0666 "$rescue_stage/dev/null" c 1 3
install -m 0755 "$script_dir/ls2k1000-rescue-init" "$rescue_stage/init"
install -m 0755 \
    "$script_dir/ls2k1000-apply-dual-root" \
    "$rescue_stage/sbin/moss-apply-dual-root"
install -m 0644 \
    "$script_dir/ls2k1000-dual-root.sfdisk" \
    "$rescue_stage/etc/moss-dual-root.sfdisk"

install -d -m 0755 "$(dirname -- "$output_image")"
(
    cd "$rescue_stage"
    find . -print0 |
        sort -z |
        cpio --null --create --format=newc --owner=0:0 2>/dev/null |
        gzip -1 >"$output_image"
)
chmod 0644 "$output_image"

echo "rescue initramfs: $output_image"
echo "size: $(stat -c %s "$output_image") bytes"
echo "sha256: $(sha256sum "$output_image" | awk '{print $1}')"
