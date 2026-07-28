#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)"
output_dir="${OUTPUT_DIR:-"$repo_dir/docs/profiling/riscv-page-fault-repro-manual"}"
source_image="${SDCARD_RV_IMG:-"$repo_dir/sdcard-rv.img"}"
binary="$output_dir/pf-repro"
console_log="$output_dir/moss-console.log"
clean_log="$output_dir/moss-console.clean.log"
trace_log="$output_dir/page-fault-trace.log"
temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/moss-pf-repro.XXXXXX")"
test_image="$temp_dir/pf-repro.img"
input_fifo="$temp_dir/qemu.in"
qemu_pid=""

cleanup() {
    if [[ -n "$qemu_pid" ]] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    rm -f -- "$test_image" "$input_fifo"
    rmdir -- "$temp_dir" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

wait_for_log() {
    local pattern="$1"
    local description="$2"
    local deadline=$((SECONDS + 120))
    while ! [[ -f "$console_log" ]] || ! rg -q "$pattern" "$console_log"; do
        if ((SECONDS >= deadline)); then
            echo "timed out waiting for $description" >&2
            return 1
        fi
        sleep 0.25
    done
}

"$repo_dir/docs/examples/page-fault-repro/build-glibc.sh"

make -C "$repo_dir" \
    ARCH=riscv64 \
    DWARF=y \
    APP_FEATURES="qemu shell-init fault-repro-trace" \
    kernel-rv

if [[ ! -f "$source_image" ]]; then
    echo "missing root filesystem image: $source_image" >&2
    exit 1
fi

cp --sparse=always -- "$source_image" "$test_image"
debugfs -w -R "write $binary /root/pf-repro" "$test_image"
debugfs -w -R "set_inode_field /root/pf-repro mode 0100755" "$test_image"

mkfifo -- "$input_fifo"
exec 3<>"$input_fifo"

qemu-system-riscv64 \
    -machine virt \
    -kernel "$repo_dir/kernel-rv" \
    -m 8G \
    -nographic \
    -smp 8 \
    -bios default \
    -drive "file=$test_image,if=none,format=raw,id=x0" \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -snapshot \
    -no-reboot \
    -rtc base=utc \
    <"$input_fifo" > >(tee "$console_log") 2>&1 &
qemu_pid=$!

wait_for_log 'starry:~#' 'the Moss shell prompt'
printf '/root/pf-repro\n' >&3
wait_for_log '\[USER\] first store returned; byte=0x5a' 'the retried store'
printf 'echo${IFS}PF_REPRO_EXIT=$?\n' >&3
wait_for_log 'PF_REPRO_EXIT=0' 'a successful process exit'
printf '\001x' >&3
wait "$qemu_pid"
qemu_pid=""

sed -E $'s/\x1B\\[[0-9;]*[[:alpha:]]//g; s/\r$//' \
    "$console_log" >"$clean_log"
rg 'PFTRACE|\[USER\]|PF_REPRO_EXIT' "$clean_log" >"$trace_log"

echo
echo "reproduction succeeded"
echo "complete console: $clean_log"
echo "fault path only: $trace_log"
