#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fixture="$repo_root/tests/fixtures/ls2k1000la.dts"
image="$repo_root/moss-2k1000la.bin"
elf="$repo_root/$(basename "$repo_root")_loongarch64-ls2k1000.elf"
dtb=$(mktemp --suffix=.ls2k1000la.dtb)

cleanup() {
    unlink -- "$dtb" 2>/dev/null || true
}
trap cleanup EXIT

for tool in dtc fdtget llvm-readelf od rg; do
    command -v "$tool" >/dev/null || {
        echo "missing required tool: $tool" >&2
        exit 1
    }
done

echo "[1/5] compile and inspect the LS2K1000LA DT fixture"
dtc -Wunique_unit_address -Wunit_address_vs_reg \
    -I dts -O dtb -o "$dtb" "$fixture"

enabled_cpus=0
while read -r cpu_node; do
    case "$cpu_node" in
        cpu@*)
            status=$(fdtget -t s "$dtb" "/cpus/$cpu_node" status 2>/dev/null || echo okay)
            if [[ "$status" != disabled ]]; then
                ((enabled_cpus += 1))
            fi
            ;;
    esac
done < <(fdtget -l "$dtb" /cpus)
[[ "$enabled_cpus" == 2 ]] || {
    echo "expected two enabled CPUs, found $enabled_cpus" >&2
    exit 1
}

[[ $(fdtget -t s "$dtb" /chosen stdout-path) == serial0:115200n8 ]]
[[ $(fdtget -t s "$dtb" /soc/serial@1fe20000 compatible) == *ns16550a* ]]
[[ $(fdtget -t s "$dtb" /soc/ahci@400e0000 compatible) == *loongson,ls-ahci* ]]
[[ $(fdtget -t s "$dtb" /soc/ethernet@40040000 status) == okay ]]
[[ $(fdtget -t s "$dtb" /soc/ethernet@40050000 status) == disabled ]]

echo "[2/5] reject fixed LS2K1000 peripheral addresses in production sources"
production_paths=(
    "$repo_root/Cargo.toml"
    "$repo_root/Makefile"
    "$repo_root/src"
    "$repo_root/kernel"
    "$repo_root/vendor/axdriver/src/ls2k1000.rs"
    "$repo_root/vendor/axdriver_net/src/dwmac_ls2k1000.rs"
    "$repo_root/vendor/axplat-loongarch64-ls2k1000"
)
if rg -n -i '0x(1fe2_?0000|1fe0_?1400|1fe0_?1040|1fe2_?7800|1fe2_?7000|400e_?0000|4004_?0000|4005_?0000)' \
    "${production_paths[@]}"; then
    echo "fixed LS2K1000 peripheral address found outside DT fixtures" >&2
    exit 1
fi

echo "[3/5] build the two-core board image"
make -C "$repo_root" kernel-la-2k1000

[[ -s "$image" ]] || {
    echo "missing board image: $image" >&2
    exit 1
}
[[ -s "$elf" ]] || {
    echo "missing board ELF: $elf" >&2
    exit 1
}

echo "[4/5] validate the boot header and ELF architecture"
[[ $(od -An -tx1 -N2 "$image" | tr -d '[:space:]') == 4d5a ]]
llvm-readelf -h "$elf" | rg -q 'Machine:[[:space:]]+LoongArch'

echo "[5/5] validate the uncached DMA section"
dma_section=$(llvm-readelf -SW "$elf" | awk '$3 == ".dma_uncached" { print; found = 1 } END { exit !found }')
[[ "$dma_section" == *" WA "* ]]
[[ "$dma_section" != *" AX "* ]]
[[ ${dma_section##* } == 4096 ]]

printf 'LS2K1000LA static checks passed: cpus=%s image=%s bytes\n' \
    "$enabled_cpus" "$(stat -c %s "$image")"
