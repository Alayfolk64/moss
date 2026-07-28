#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)"
output_dir="${OUTPUT_DIR:-"$repo_dir/docs/profiling/riscv-page-fault-repro-manual"}"
testsuites_dir="${TESTSUITES_DIR:-"$repo_dir/../testsuits-for-oskernel"}"
glibc_dir="${GLIBC_DIR:-"$testsuites_dir/sdcard/riscv/glibc/lib"}"
cross_cc="${RISCV_CC:-riscv64-linux-musl-gcc}"
cross_objdump="${RISCV_OBJDUMP:-riscv64-linux-musl-objdump}"
source_file="$repo_dir/docs/examples/page-fault-repro/pf-repro.c"
binary="$output_dir/pf-repro"

for path in \
    "$glibc_dir/libc.so.6" \
    "$glibc_dir/ld-linux-riscv64-lp64d.so.1"; do
    if [[ ! -f "$path" ]]; then
        echo "missing RISC-V glibc runtime: $path" >&2
        exit 1
    fi
done

mkdir -p -- "$output_dir"

"$cross_cc" \
    -march=rv64gc -mabi=lp64d -O2 \
    -fno-pie -no-pie -nostdlib -fno-stack-protector \
    -Wl,-e,_start \
    -Wl,--dynamic-linker=/glibc/lib/ld-linux-riscv64-lp64d.so.1 \
    -Wl,-rpath=/glibc/lib \
    -Wl,--allow-shlib-undefined \
    "$source_file" \
    -L"$glibc_dir" \
    -Wl,--no-as-needed -Wl,-l:libc.so.6 \
    -o "$binary"

readelf -lW "$binary" |
    rg 'INTERP|Requesting' >"$output_dir/elf-interpreter.txt"
readelf -dW "$binary" |
    rg 'NEEDED|RPATH|RUNPATH' >"$output_dir/elf-dynamic.txt"
readelf -sW "$binary" |
    rg ' (mmap|write|munmap|_exit)@' >"$output_dir/glibc-symbols.txt"
"$cross_objdump" -d "$binary" |
    sed -n '/<first_store/,/ret/p' >"$output_dir/faulting-instruction.txt"

echo "built: $binary"
echo "glibc evidence: $output_dir/elf-interpreter.txt"
echo "faulting instruction: $output_dir/faulting-instruction.txt"
