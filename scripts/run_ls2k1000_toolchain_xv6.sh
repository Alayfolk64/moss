#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project_root=$(dirname "$repo_root")
serial_device=/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0
board_host_ip=192.168.88.1
apk_repo_port=18083
xv6_url=https://github.com/mit-pdos/xv6-riscv.git
xv6_commit=7d01506ee03394b6b1a161ccee69cd150ab471fc
xv6_sha256=3678060565355c13c68c1916a3cda3e5839b3ed9f6580312bb634172433942f6
xv6_source="$project_root/xv6-riscv-upstream"

mkdir -p "$repo_root/tmp"
export TMPDIR="$repo_root/tmp"

for tool in awk curl docker git ip make python3 sha256sum sudo; do
    command -v "$tool" >/dev/null || {
        echo "missing host tool: $tool" >&2
        exit 1
    }
done

board_interface=$(
    ip -o -4 address show |
        awk -v address="$board_host_ip" \
            '{ split($4, fields, "/") } fields[1] == address { print $2; exit }'
)
[[ -n "$board_interface" ]] || {
    echo "host does not own $board_host_ip" >&2
    exit 1
}
[[ -e "$serial_device" ]] || {
    echo "serial device is missing: $serial_device" >&2
    exit 1
}
[[ -f /srv/tftp/board.dtb ]] || {
    echo "TFTP DTB is missing: /srv/tftp/board.dtb" >&2
    exit 1
}

if [[ ! -d "$xv6_source/.git" ]]; then
    git clone "$xv6_url" "$xv6_source"
fi
if ! git -C "$xv6_source" cat-file -e "$xv6_commit^{commit}"; then
    git -C "$xv6_source" fetch origin "$xv6_commit"
fi
actual_xv6_commit=$(git -C "$xv6_source" rev-parse "$xv6_commit^{commit}")
[[ "$actual_xv6_commit" == "$xv6_commit" ]] || {
    echo "xv6 commit mismatch: $actual_xv6_commit" >&2
    exit 1
}

package_cache="$repo_root/tmp/ls2k-toolchain-packages"
local_repo_root="$repo_root/tmp/ls2k-toolchain-http"
xv6_archive="$local_repo_root/xv6-riscv-7d01506.tar.gz"
offline_bundle="$local_repo_root/toolchain-offline.tar"
mkdir -p "$package_cache" "$local_repo_root"
find "$package_cache" -maxdepth 1 -type f -name '*.apk' -delete

docker run --rm -v "$package_cache:/packages" alpine:3.21 sh -ec \
    "printf '%s\\n' http://mirrors.aliyun.com/alpine/v3.21/main http://mirrors.aliyun.com/alpine/v3.21/community > /tmp/repositories; apk --allow-untrusted --no-cache --arch loongarch64 --repositories-file /tmp/repositories fetch --recursive --output /packages g++ clang19 llvm19 lld perl curl git rust cargo"
xv6_archive_new="$xv6_archive.new"
git -C "$xv6_source" archive --format=tar.gz \
    --prefix=xv6-riscv-7d01506/ \
    --output="$xv6_archive_new" "$xv6_commit"
actual_xv6_sha=$(sha256sum "$xv6_archive_new" | awk '{ print $1 }')
[[ "$actual_xv6_sha" == "$xv6_sha256" ]] || {
    echo "xv6 archive hash mismatch: $actual_xv6_sha" >&2
    exit 1
}
mv "$xv6_archive_new" "$xv6_archive"

for package in g++ clang19 llvm19 lld perl curl git rust cargo; do
    find "$package_cache" -maxdepth 1 -type f -name "$package-*.apk" \
        -print -quit | grep -q . || {
        echo "required APK is missing: $package" >&2
        exit 1
    }
done
mapfile -t offline_packages < <(
    find "$package_cache" -maxdepth 1 -type f -name '*.apk' -printf '%f\n' |
        sort
)
[[ ${#offline_packages[@]} -ge 40 ]] || {
    echo "unexpected toolchain APK closure: ${#offline_packages[@]} packages" >&2
    exit 1
}
tar -C "$package_cache" -cf "$offline_bundle" "${offline_packages[@]}"

http_server_pid=
cleanup() {
    if [[ -n "$http_server_pid" ]]; then
        kill "$http_server_pid" 2>/dev/null || true
        wait "$http_server_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

http_log="$repo_root/tmp/ls2k-toolchain-http-server.log"
python3 -m http.server "$apk_repo_port" --bind "$board_host_ip" \
    --directory "$local_repo_root" >"$http_log" 2>&1 &
http_server_pid=$!
for _ in 1 2 3 4 5; do
    if curl --noproxy '*' -fsS \
        "http://$board_host_ip:$apk_repo_port/$(basename "$offline_bundle")" \
        -o /dev/null 2>/dev/null; then
        break
    fi
    sleep 1
done
kill -0 "$http_server_pid"
curl --noproxy '*' -fsS \
    "http://$board_host_ip:$apk_repo_port/$(basename "$xv6_archive")" \
    -o /dev/null

image="$repo_root/moss-2k1000la.bin"
if [[ ${LS2K_TOOLCHAIN_SKIP_BUILD:-0} != 1 ]]; then
    make -C "$repo_root" SITE_INIT=y kernel-la-2k1000
fi
[[ -f "$image" ]] || {
    echo "MOSS image is missing: $image" >&2
    exit 1
}
image_sha=$(sha256sum "$image" | awk '{ print $1 }')
tftp_name="moss-toolchain-xv6-${image_sha:0:12}.bin"
tftp_target="/srv/tftp/$tftp_name"
sudo install -o tftp -g tftp -m 0644 "$image" "$tftp_target"
tftp_sha=$(sha256sum "$tftp_target" | awk '{ print $1 }')
[[ "$image_sha" == "$tftp_sha" ]] || {
    echo "TFTP image hash mismatch" >&2
    exit 1
}

timestamp=$(date +%Y%m%d-%H%M%S)
log="$repo_root/tmp/hardware-ls2k1000la-toolchain-xv6-$timestamp.log"
watchdog_hard_deadline=${LS2K_WATCHDOG_HARD_DEADLINE:-2700}
reset_timeout=$((watchdog_hard_deadline + 60))
printf 'image=%s\nsha256=%s\nxv6_commit=%s\nxv6_sha256=%s\nlog=%s\n' \
    "$tftp_name" "$image_sha" "$xv6_commit" "$xv6_sha256" "$log"

python3 "$repo_root/scripts/run_ls2k1000_serial_tftp.py" "$tftp_name" \
    --root-device /dev/sda3 \
    --device "$serial_device" \
    --test toolchain-xv6 \
    --watchdog-hard-deadline-seconds "$watchdog_hard_deadline" \
    --reset-timeout "$reset_timeout" \
    --log "$log"
