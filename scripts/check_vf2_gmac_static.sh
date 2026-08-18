#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mainline_fixture="$repo_root/tests/fixtures/visionfive2-v13b-mainline.dts"
legacy_fixture="$repo_root/tests/fixtures/visionfive2-v13b-uboot-2021.10.dts"
image="$repo_root/build/site-vf2/moss-vf2.bin"
elf="$repo_root/build/site-vf2/moss-dev_visionfive2.elf"
tmp_dir=$(mktemp -d --suffix=.vf2-gmac)

cleanup() {
    rm -r -- "$tmp_dir"
}
trap cleanup EXIT

for tool in dtc fdtget llvm-readelf make python3 rg rustc; do
    command -v "$tool" >/dev/null || {
        echo "missing required tool: $tool" >&2
        exit 1
    }
done

echo "[1/5] compile both supported VisionFive 2 v1.3B DT contracts"
dtc -I dts -O dtb -o "$tmp_dir/mainline.dtb" "$mainline_fixture"
dtc -I dts -O dtb -o "$tmp_dir/legacy.dtb" "$legacy_fixture" 2>"$tmp_dir/legacy.warn"

mainline_node=/soc/ethernet@16040000
legacy_node=/soc/ethernet@16040000
[[ $(fdtget -t s "$tmp_dir/mainline.dtb" "$mainline_node" compatible) == *starfive,jh7110-dwmac* ]]
[[ $(fdtget -t s "$tmp_dir/mainline.dtb" "$mainline_node" phy-mode) == rgmii-id ]]
[[ $(fdtget -t i "$tmp_dir/mainline.dtb" "$mainline_node" interrupts) == "78 77 76" ]]
[[ $(fdtget -t i "$tmp_dir/mainline.dtb" "$mainline_node" snps,txpbl) == 16 ]]
[[ $(fdtget -t i "$tmp_dir/mainline.dtb" "$mainline_node" snps,rxpbl) == 16 ]]
read -r -a mainline_clocks <<<"$(fdtget -t i "$tmp_dir/mainline.dtb" "$mainline_node" clocks)"
[[ ${#mainline_clocks[@]} == 10 ]]
[[ ${mainline_clocks[1]} == 98 && ${mainline_clocks[3]} == 97 ]]
[[ ${mainline_clocks[5]} == 102 && ${mainline_clocks[7]} == 106 ]]
[[ ${mainline_clocks[9]} == 107 ]]
read -r -a mainline_resets <<<"$(fdtget -t i "$tmp_dir/mainline.dtb" "$mainline_node" resets)"
[[ ${#mainline_resets[@]} == 4 ]]
[[ ${mainline_resets[1]} == 66 && ${mainline_resets[3]} == 67 ]]
fdtget "$tmp_dir/mainline.dtb" "$mainline_node" starfive,syscon >/dev/null
fdtget "$tmp_dir/mainline.dtb" "$mainline_node/mdio/ethernet-phy@0" motorcomm,tx-clk-adj-enabled >/dev/null
fdtget "$tmp_dir/mainline.dtb" "$mainline_node/mdio/ethernet-phy@0" motorcomm,tx-clk-100-inverted >/dev/null

[[ $(fdtget -t s "$tmp_dir/legacy.dtb" "$legacy_node" compatible) == *starfive,jh7110-eqos-5.20* ]]
[[ $(fdtget -t s "$tmp_dir/legacy.dtb" "$legacy_node" phy-mode) == rgmii-id ]]
if fdtget "$tmp_dir/legacy.dtb" "$legacy_node" phy-handle >/dev/null 2>&1; then
    echo "legacy fixture must exercise the missing phy-handle fallback" >&2
    exit 1
fi
[[ $(fdtget -t i "$tmp_dir/legacy.dtb" "$legacy_node/mdio/ethernet-phy@1" tx_inverted_100) == 1 ]]
[[ $(fdtget -t i "$tmp_dir/legacy.dtb" "$legacy_node/mdio/ethernet-phy@1" tx_inverted_1000) == 0 ]]

echo "[2/5] run shared and JH7110 host-side state tests"
mkdir -p "$repo_root/target/driver-tests"
rustc --edition 2024 --test -A dead-code \
    "$repo_root/vendor/axdriver_net/src/dwmac_common_state.rs" \
    -o "$repo_root/target/driver-tests/dwmac-common-state"
"$repo_root/target/driver-tests/dwmac-common-state" --quiet
rustc --edition 2024 --test -A dead-code \
    "$repo_root/vendor/axdriver_net/src/dwmac_jh7110_state.rs" \
    -o "$repo_root/target/driver-tests/dwmac-jh7110-state"
"$repo_root/target/driver-tests/dwmac-jh7110-state" --quiet

echo "[3/5] enforce the VF2 lifecycle and PHY-management source contracts"
driver="$repo_root/vendor/axdriver_net/src/dwmac_jh7110.rs"
platform="$repo_root/vendor/axdriver/src/jh7110.rs"
rg -q 'APP_FEATURES="vf2 site-init"' "$repo_root/Makefile"
rg -q 'fn ensure_started' "$driver"
rg -q 'register_interrupt_handler' "$driver" "$platform"
rg -q 'fn mdio_write' "$driver"
rg -q 'YT8531_RGMII_CONFIG1' "$driver"
rg -q 'rx_descriptor_disposition' "$driver"
rg -q 'recover_dma' "$driver"
rg -q 'mac_address_high' "$driver"
rg -q 'mac_packet_filter' "$driver"
rg -q 'starfive,jh7110-eqos-5\.20' "$platform"
rg -q 'RMII/v1\.2A is unsupported' "$platform"
rg -q 'fn legacy_phy_node' "$platform"
if rg -n 'get_node_by_name\("ethernet-phy' "$platform"; then
    echo "legacy PHY lookup must stay inside the GMAC1 subtree" >&2
    exit 1
fi
python3 -c 'compile(open(__import__("sys").argv[1], encoding="utf-8").read(), __import__("sys").argv[1], "exec")' \
    "$repo_root/scripts/run_vf2_gmac_test.py"
if rg -n 'dev\.start_dma\(\)' "$driver"; then
    echo "VF2 probe must not start DMA" >&2
    exit 1
fi
if rg -n 'MAC_PACKET_FILTER_PR' "$driver"; then
    echo "VF2 must not leave GMAC1 in permanent promiscuous mode" >&2
    exit 1
fi

echo "[4/5] build the VF2 site image"
make -C "$repo_root" site-vf2
[[ -s "$image" ]] || {
    echo "missing VF2 image: $image" >&2
    exit 1
}
[[ -s "$elf" ]] || {
    echo "missing VF2 ELF: $elf" >&2
    exit 1
}
llvm-readelf -h "$elf" | rg -q 'Machine:[[:space:]]+RISC-V'
rg -a -q 'jh7110-gmac1' "$elf"

echo "[5/5] validate non-coherent DMA section placement"
dma_section=$(
    llvm-readelf -SW "$elf" |
        awk '$2 == ".dma_uncached" || $3 == ".dma_uncached" { print; found = 1 } END { exit !found }'
)
[[ "$dma_section" == *" WA "* ]]
[[ "$dma_section" != *" AX "* ]]

printf 'VF2 GMAC static checks passed: image=%s bytes, mainline+legacy DT\n' \
    "$(stat -c %s "$image")"
