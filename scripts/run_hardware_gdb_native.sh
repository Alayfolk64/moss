#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "$repo_root/tmp"
export TMPDIR="$repo_root/tmp"

usage() {
    echo "usage: $0 {vf2|ls2k1000la|all}" >&2
}

[[ $# -eq 1 ]] || { usage; exit 2; }
requested_board=$1
case "$requested_board" in
    vf2|ls2k1000la|all) ;;
    *) usage; exit 2 ;;
esac

for tool in awk date docker find grep ip make python3 sha256sum sort sudo tar wc; do
    command -v "$tool" >/dev/null || {
        echo "missing host tool: $tool" >&2
        exit 1
    }
done
sudo -n true

prepare_bundle() {
    local release=$1 apk_arch=$2 cache bundle ready version package_count
    cache="$repo_root/tmp/qemu-gdb-packages-$release-$apk_arch"
    bundle="$repo_root/tmp/hardware-gdb-native-$apk_arch.tar"
    ready="$cache/.ready"
    version="gdb-native-v1-$release-$apk_arch"
    mkdir -p "$cache"
    if [[ ! -f "$ready" || $(<"$ready") != "$version" ]]; then
        find "$cache" -maxdepth 1 -type f -name '*.apk' -delete
        docker run --rm -v "$cache:/packages" "alpine:$release" sh -ec \
            "printf '%s\\n' http://mirrors.aliyun.com/alpine/v$release/main http://mirrors.aliyun.com/alpine/v$release/community > /tmp/repositories; apk --allow-untrusted --no-cache --arch '$apk_arch' --repositories-file /tmp/repositories fetch --recursive --output /packages gdb gcc musl-dev binutils file" \
            1>&2
        printf '%s\n' "$version" >"$ready"
    fi
    for package in gdb gcc musl-dev binutils file; do
        find "$cache" -maxdepth 1 -type f -name "$package-[0-9]*.apk" \
            -print -quit | grep -q . || {
            echo "required APK missing for $apk_arch: $package" >&2
            return 1
        }
    done
    package_count=$(find "$cache" -maxdepth 1 -type f -name '*.apk' | wc -l)
    [[ "$package_count" -ge 15 ]] || {
        echo "unexpected GDB APK closure for $apk_arch: $package_count" >&2
        return 1
    }
    mapfile -t packages < <(
        find "$cache" -maxdepth 1 -type f -name '*.apk' -printf '%f\n' | sort
    )
    tar -C "$cache" -cf "$bundle" "${packages[@]}"
    tar -C "$repo_root/tests" -rf "$bundle" \
        gdb-native-demo.c gdb-native-smoke.gdb
    printf '%s %s %s\n' "$bundle" "$package_count" \
        "$(sha256sum "$bundle" | awk '{print $1}')"
}

run_vf2() {
    local serial_arg=() interface_arg=() bundle_info bundle package_count
    if [[ -n ${VF2_SERIAL:-} ]]; then
        serial_arg=(--serial "$VF2_SERIAL")
        [[ -e "$VF2_SERIAL" ]] || {
            echo "VF2 serial device is missing: $VF2_SERIAL" >&2
            return 2
        }
    elif [[ ! -e /dev/ttyACM0 ]] && \
        ! compgen -G '/dev/serial/by-id/usb-1a86_USB_Single_Serial_*-if00' \
            >/dev/null; then
        echo "VF2 serial device is not connected" >&2
        return 2
    fi
    if [[ -n ${VF2_INTERFACE:-} ]]; then
        interface_arg=(--interface "$VF2_INTERFACE")
    fi
    bundle_info=$(prepare_bundle 3.22 riscv64) || return $?
    read -r bundle package_count _ <<<"$bundle_info"
    if [[ ${VF2_GDB_SKIP_BUILD:-0} != 1 ]]; then
        make -B -C "$repo_root" SITE_INIT=y kernel-vf2 || return $?
    fi
    [[ -s "$repo_root/moss-vf2.bin" ]]
    local timestamp log
    timestamp=$(date +%Y%m%d-%H%M%S)
    log="$repo_root/tmp/hardware-vf2-gdb-native-$timestamp.log"
    python3 "$repo_root/scripts/run_vf2_gdb_native.py" \
        --image "$repo_root/moss-vf2.bin" --bundle "$bundle" \
        --package-floor "$package_count" --log "$log" \
        "${serial_arg[@]}" "${interface_arg[@]}" || return $?
    echo "[hardware-gdb] VF2 log=$log"
}

run_ls2k1000la() {
    local device=${LS2K_GDB_SERIAL:-/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0}
    [[ -e "$device" ]] || {
        echo "LS2K1000LA serial device is not connected: $device" >&2
        return 2
    }
    [[ -f /srv/tftp/board.dtb ]] || {
        echo "LS2K1000LA TFTP DTB is missing: /srv/tftp/board.dtb" >&2
        return 2
    }
    ip -o -4 address show | grep -q '192\.168\.88\.1/' || {
        echo "host does not own LS2K1000LA address 192.168.88.1" >&2
        return 2
    }
    local bundle_info bundle package_count
    bundle_info=$(prepare_bundle 3.21 loongarch64) || return $?
    read -r bundle package_count _ <<<"$bundle_info"
    if [[ ${LS2K_GDB_SKIP_BUILD:-0} != 1 ]]; then
        make -B -C "$repo_root" SITE_INIT=y kernel-la-2k1000 || return $?
    fi
    [[ -s "$repo_root/moss-2k1000la.bin" ]]
    local image_sha tftp_name timestamp log
    image_sha=$(sha256sum "$repo_root/moss-2k1000la.bin" | awk '{print $1}')
    tftp_name="moss-ls2k1000la-gdb-${image_sha:0:16}.bin"
    sudo install -o tftp -g tftp -m 0644 \
        "$repo_root/moss-2k1000la.bin" "/srv/tftp/$tftp_name"
    [[ $(sha256sum "/srv/tftp/$tftp_name" | awk '{print $1}') == "$image_sha" ]]
    timestamp=$(date +%Y%m%d-%H%M%S)
    log="$repo_root/tmp/hardware-ls2k1000la-gdb-native-$timestamp.log"
    python3 "$repo_root/scripts/run_ls2k1000_gdb_native.py" \
        --image "$tftp_name" --bundle "$bundle" \
        --package-floor "$package_count" --device "$device" --log "$log" \
        || return $?
    echo "[hardware-gdb] LS2K1000LA log=$log"
}

case "$requested_board" in
    vf2) run_vf2 ;;
    ls2k1000la) run_ls2k1000la ;;
    all)
        failed=0
        run_vf2 || failed=1
        run_ls2k1000la || failed=1
        exit "$failed"
        ;;
esac
