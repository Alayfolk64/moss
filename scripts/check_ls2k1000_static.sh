#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
app_name=$(basename "$repo_root")
fixture="$repo_root/tests/fixtures/ls2k1000la.dts"
image="$repo_root/build/site-2kla/moss-2kla.bin"
elf="$repo_root/build/site-2kla/${app_name}_loongarch64-ls2k1000.elf"
mkdir -p "$repo_root/tmp"
dtb=$(mktemp -p "$repo_root/tmp" --suffix=.ls2k1000la.dtb)

cleanup() {
    unlink -- "$dtb" 2>/dev/null || true
}
trap cleanup EXIT

for tool in dtc fdtget llvm-objdump llvm-readelf make od rg rustc; do
    command -v "$tool" >/dev/null || {
        echo "missing required tool: $tool" >&2
        exit 1
    }
done

echo "[1/6] compile and inspect the LS2K1000LA DT fixture"
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
[[ $(fdtget -t s "$dtb" /soc/ahci@400e0000 compatible) == *snps,spear-ahci* ]]
[[ $(fdtget -t i "$dtb" /soc/ahci@400e0000 interrupts) == 19 ]]
[[ $(fdtget -t i "$dtb" /soc/ahci@400e0000 ports-implemented) == 1 ]]
[[ $(fdtget -t s "$dtb" /soc/ethernet@40040000 status) == okay ]]
[[ $(fdtget -t i "$dtb" /soc/ethernet@40040000 interrupts) == 12 ]]
[[ $(fdtget -t s "$dtb" /soc/ethernet@40040000 phy-mode) == rgmii ]]
fdtget -t x "$dtb" /soc/ethernet@40040000 phy-handle >/dev/null
[[ $(fdtget -t s "$dtb" /soc/ethernet@40040000/mdio/ethernet-phy@0 compatible) == \
    ethernet-phy-ieee802.3-c22 ]]
[[ $(fdtget -t i "$dtb" /soc/ethernet@40040000/mdio/ethernet-phy@0 reg) == 0 ]]
[[ $(fdtget -t i "$dtb" /soc/ethernet@40040000 snps,pbl) == 8 ]]
[[ $(fdtget -t i "$dtb" /soc/ethernet@40040000 snps,txpbl) == 16 ]]
[[ $(fdtget -t i "$dtb" /soc/ethernet@40040000 snps,rxpbl) == 8 ]]
fdtget "$dtb" /soc/ethernet@40040000 snps,mixed-burst >/dev/null
[[ $(fdtget -t i "$dtb" /stmmac-axi-config snps,wr_osr_lmt) == 1 ]]
[[ $(fdtget -t i "$dtb" /stmmac-axi-config snps,rd_osr_lmt) == 2 ]]
[[ $(fdtget -t i "$dtb" /stmmac-axi-config snps,blen) == "4 8 16 32 0 0 0" ]]
if fdtget "$dtb" /stmmac-axi-config compatible >/dev/null 2>&1; then
    echo "Linux DWMAC AXI subnode must not carry a compatible property" >&2
    exit 1
fi
[[ $(fdtget -t s "$dtb" /soc/ethernet@40050000 status) == disabled ]]
[[ $(fdtget -t s "$dtb" /soc/rtc@1fe27800 status) == okay ]]
[[ $(fdtget -t i "$dtb" /soc/rtc@1fe27800 interrupts) == 20 ]]
[[ $(fdtget -t s "$dtb" /soc/watchdog@1fe27030 compatible) == loongson,ls2x-wdt ]]
[[ $(fdtget -t i "$dtb" /soc/watchdog@1fe27030 reg) == "0 534933552 0 12" ]]
[[ $(fdtget -t s "$dtb" /soc/watchdog@1fe27030 clock-names) == wdt-clk ]]
[[ $(fdtget -t s "$dtb" /reboot compatible) == syscon-reboot ]]
[[ $(fdtget -t s "$dtb" /poweroff compatible) == syscon-poweroff ]]

echo "[2/6] run host-side state-machine tests"

rg -q 'pub time: SpinNoIrq<TimeManager>' "$repo_root/kernel/src/task/mod.rs"
rg -q 'static ref ALARM_RUNTIME: SpinNoIrq<AlarmRuntime>' \
    "$repo_root/kernel/src/task/timer.rs"
rg -Fq 'timers: Vec<(TimerKey, Entry)>' "$repo_root/kernel/src/task/timer.rs"
if rg -q 'BTreeMap<TimerKey, Entry>' "$repo_root/kernel/src/task/timer.rs"; then
    echo "LS2K1000 alarm queue must not use the repeatedly corrupted BTreeMap path" >&2
    exit 1
fi
rg -q 'MOSS_NTP_SYNC:-0' "$repo_root/src/init_shell.sh"
rg -q 'const SIOCGIFCONF: u32 = 0x8912' "$repo_root/kernel/src/file/net.rs"
rg -Fq 'root.add("net"' "$repo_root/kernel/src/pseudofs/proc.rs"
rg -q '^site-2kla:$' "$repo_root/Makefile"
rg -q 'APP_FEATURES="ls2k1000 site-init ls2k1000-shell-mode"' \
    "$repo_root/Makefile"
rg -q 'ARCH=loongarch64 BUS=mmio MEM=2G CPU_CAPACITY=2' "$repo_root/Makefile"
rg -q "^export PS1='starry:~# '$" "$repo_root/src/init_site.sh"
rg -q '^cd /root 2>/dev/null \|\| cd /$' "$repo_root/src/init_site.sh"
[[ $(tail -n 1 "$repo_root/src/init_site.sh") == 'exec /bin/sh' ]]
if rg -q '^kernel-la-2k1000(-shell)?:' "$repo_root/Makefile"; then
    echo "legacy LS2K1000LA image target must not remain in Makefile" >&2
    exit 1
fi
rg -q 'MOSS_NET GMAC_RESET_TIMEOUT' \
    "$repo_root/vendor/axdriver_net/src/dwmac_ls2k1000.rs"
rg -q 'MOSS_NET GMAC_READY' "$repo_root/vendor/axdriver_net/src/dwmac_ls2k1000.rs"
if rg -q 'AssumeSync<RefCell<TimeManager>>' "$repo_root/kernel/src/task/mod.rs"; then
    echo "thread timer state must not use an unsynchronized RefCell on SMP" >&2
    exit 1
fi

test_bin_dir="$repo_root/target/driver-tests"
mkdir -p "$test_bin_dir"
state_tests=(
    "vendor/axplat/src/console_rx.rs:console-rx-buffer-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/boot_arg.rs:ls2k1000-boot-arg-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/node_clock_state.rs:ls2k1000-node-clock-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/rtc_state.rs:ls2k1000-rtc-state-tests"
    "vendor/fdt-parser/src/lib.rs:fdt-parser-tests"
    "vendor/axhal/src/ram_source.rs:ram-source-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/mem_layout.rs:ls2k1000-memory-layout-tests"
    "vendor/simple-ahci/src/hal.rs:ahci-wait-policy-tests"
    "vendor/simple-ahci/src/host_reset.rs:ahci-host-reset-tests"
    "vendor/simple-ahci/src/port_state.rs:ahci-port-state-tests"
    "vendor/simple-ahci/src/completion.rs:ahci-completion-tests"
    "vendor/axtask/src/block_context.rs:task-block-context-tests"
    "vendor/axdriver_block/src/block_range.rs:block-range-tests"
    "vendor/axdriver_net/src/dwmac_common_state.rs:dwmac-common-state-tests"
    "vendor/axdriver_net/src/dwmac_ls2k1000_state.rs:dwmac-state-tests"
    "vendor/axdriver/src/ls2k1000_irq.rs:ls2k1000-irq-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/irq_state.rs:liointc-state-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/ipi_state.rs:ipi-state-tests"
)
for entry in "${state_tests[@]}"; do
    source_file=${entry%%:*}
    test_name=${entry##*:}
    rustc --edition 2024 --test -A dead-code \
        "$repo_root/$source_file" -o "$test_bin_dir/$test_name"
    "$test_bin_dir/$test_name" --quiet
done

echo "[3/6] reject fixed LS2K1000 peripheral addresses in production sources"
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

echo "[4/6] build and validate the site image"
make -C "$repo_root" site-2kla
[[ -s "$image" ]] || {
    echo "missing board image: $image" >&2
    exit 1
}
[[ -s "$elf" ]] || {
    echo "missing board ELF: $elf" >&2
    exit 1
}
[[ $(od -An -tx1 -N2 "$image" | tr -d '[:space:]') == 4d5a ]]
llvm-readelf -h "$elf" | rg -q 'Machine:[[:space:]]+LoongArch'

echo "[5/6] validate the LoongArch boot and context contracts"
boot_entry_disasm=$(
    llvm-objdump -d --demangle "$elf" |
        sed -n '/<_stext>:/,/^$/p' |
        head -n 20
)
rg -q 'ori[[:space:]]+\$t0, \$zero, 4' <<<"$boot_entry_disasm"
rg -q 'csrxchg[[:space:]]+\$zero, \$t0, 0' <<<"$boot_entry_disasm"

context_switch_disasm=$(
    llvm-objdump -d --demangle "$elf" |
        sed -n '/<axcpu::loongarch64::context::context_switch/,/^$/p'
)
[[ -n "$context_switch_disasm" ]] || {
    echo "missing LoongArch context_switch in board ELF" >&2
    exit 1
}
rg -q 'csrrd[[:space:]]+\$t0, 1' <<<"$context_switch_disasm"
rg -q 'csrwr[[:space:]]+\$t0, 1' <<<"$context_switch_disasm"

mmu_setup_disasm=$(
    llvm-objdump -d --demangle "$elf" |
        sed -n '/<axcpu::loongarch64::init::init_mmu/,/^$/p'
)
[[ -n "$mmu_setup_disasm" ]] || {
    echo "missing LoongArch init_mmu in board ELF" >&2
    exit 1
}
rg -q 'ori[[:space:]]+\$a0, \$zero, 176' <<<"$mmu_setup_disasm"
rg -q 'csrwr[[:space:]]+\$a0, 0' <<<"$mmu_setup_disasm"

echo "[6/6] validate DTB parsing and uncached DMA layout"
fdt_from_bytes_disasm=$(
    llvm-objdump -d --demangle "$elf" |
        sed -n '/<fdt_parser::fdt::Fdt::from_bytes/,/^$/p'
)
[[ -n "$fdt_from_bytes_disasm" ]] || {
    echo "missing fdt_parser::Fdt::from_bytes in board ELF" >&2
    exit 1
}
while read -r offset; do
    if ((offset % 8 != 0)); then
        echo "unaligned dword access at offset $offset in Fdt::from_bytes" >&2
        exit 1
    fi
done < <(
    sed -nE 's/.*(ld|st)\.d[^,]*,[[:space:]]*\$[^,]+,[[:space:]]*(-?[0-9]+)([[:space:]].*)?$/\2/p' \
        <<<"$fdt_from_bytes_disasm"
)

fdt_from_ptr_disasm=$(
    llvm-objdump -d --demangle "$elf" |
        sed -n '/<fdt_parser::fdt::Fdt::from_ptr/,/^$/p'
)
[[ -n "$fdt_from_ptr_disasm" ]] || {
    echo "missing fdt_parser::Fdt::from_ptr in board ELF" >&2
    exit 1
}
if rg -q 'ld\.(h|hu|w|wu|d)[[:space:]].*, (7|15|23|31)$' <<<"$fdt_from_ptr_disasm"; then
    echo "unaligned wide load found in firmware DTB header path" >&2
    exit 1
fi

dma_section=$(
    llvm-readelf -SW "$elf" |
        awk '$2 == ".dma_uncached" || $3 == ".dma_uncached" { print; found = 1 } END { exit !found }'
)
[[ "$dma_section" == *" WA "* ]]
[[ "$dma_section" != *" AX "* ]]
[[ ${dma_section##* } == 4096 ]]

printf 'LS2K1000LA site static checks passed: cpus=%s image=%s bytes\n' \
    "$enabled_cpus" "$(stat -c %s "$image")"
