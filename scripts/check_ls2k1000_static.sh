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

for tool in cargo dtc fdtget llvm-objdump llvm-readelf od rg rustc; do
    command -v "$tool" >/dev/null || {
        echo "missing required tool: $tool" >&2
        exit 1
    }
done

echo "[1/8] compile and inspect the LS2K1000LA DT fixture"
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

echo "[2/8] run host-side peripheral state-machine tests"
test_bin_dir="$repo_root/target/driver-tests"
mkdir -p "$test_bin_dir"

# As in Linux start_secondary(), configure per-CPU platform state while global
# interrupts remain disabled and enable local IRQs only after the scheduler,
# runtime IPI queue and online publication are complete.
secondary_bootstrap_impl=$(
    sed -n '/pub fn rust_main_secondary/,/^}/p' \
        "$repo_root/vendor/axruntime/src/mp.rs"
)
secondary_percpu_line=$(rg -n 'percpu::init_secondary' <<<"$secondary_bootstrap_impl" | cut -d: -f1 || true)
secondary_early_line=$(rg -n 'axhal::init_early_secondary' <<<"$secondary_bootstrap_impl" | cut -d: -f1 || true)
secondary_scheduler_line=$(rg -n 'axtask::init_scheduler_secondary' <<<"$secondary_bootstrap_impl" | cut -d: -f1 || true)
secondary_ipi_line=$(rg -n 'axipi::init\(\)' <<<"$secondary_bootstrap_impl" | cut -d: -f1 || true)
secondary_platform_line=$(rg -n 'axhal::init_later_secondary' <<<"$secondary_bootstrap_impl" | cut -d: -f1 || true)
secondary_online_line=$(rg -n 'INITED_CPUS\.fetch_add' <<<"$secondary_bootstrap_impl" | cut -d: -f1 || true)
secondary_irq_enable_line=$(rg -n 'axhal::asm::enable_irqs' <<<"$secondary_bootstrap_impl" | cut -d: -f1 || true)
if [[ -z "$secondary_percpu_line" || -z "$secondary_early_line" \
    || -z "$secondary_scheduler_line" || -z "$secondary_ipi_line" \
    || -z "$secondary_platform_line" || -z "$secondary_online_line" \
    || -z "$secondary_irq_enable_line" ]] \
    || ((secondary_early_line <= secondary_percpu_line)) \
    || ((secondary_platform_line <= secondary_early_line)) \
    || ((secondary_scheduler_line <= secondary_platform_line)) \
    || ((secondary_ipi_line <= secondary_scheduler_line)) \
    || ((secondary_online_line <= secondary_ipi_line)) \
    || ((secondary_irq_enable_line <= secondary_online_line)); then
    echo "secondary CPU state must be online before local interrupts are enabled" >&2
    exit 1
fi

# Keep Linux's action split connected to the hardware paths: action 0 wakes a
# CPU, while action 1 represents an `axipi` runtime queue notification. The
# handler must acknowledge status before it decides whether to enter `axipi`.
ls2k_boot_ipi_impl=$(<"$repo_root/vendor/axplat-loongarch64-ls2k1000/src/mp.rs")
ls2k_runtime_ipi_impl=$(<"$repo_root/vendor/axplat-loongarch64-ls2k1000/src/irq.rs")
if ! rg -q 'send_ipi_single\(cpu_id, BOOT_ACTION\)' <<<"$ls2k_boot_ipi_impl" \
    || ! rg -q 'send_ipi_single\(hardware_id, RUNTIME_ACTION\)' <<<"$ls2k_runtime_ipi_impl"; then
    echo "LS2K1000 boot and runtime IPIs must use distinct action vectors" >&2
    exit 1
fi
ls2k_handle_ipi_impl=$(
    sed -n '/fn handle_ipi()/,/^}/p' \
        "$repo_root/vendor/axplat-loongarch64-ls2k1000/src/irq.rs"
)
ipi_clear_line=$(rg -n 'iocsr_write_w\(IOCSR_IPI_CLEAR, status\)' \
    <<<"$ls2k_handle_ipi_impl" | cut -d: -f1 || true)
ipi_decode_line=$(rg -n 'decode_pending\(status\)' \
    <<<"$ls2k_handle_ipi_impl" | cut -d: -f1 || true)
ipi_dispatch_line=$(rg -n 'IRQ_HANDLERS\.handle\(IPI_IRQ\)' \
    <<<"$ls2k_handle_ipi_impl" | cut -d: -f1 || true)
if [[ -z "$ipi_clear_line" || -z "$ipi_decode_line" || -z "$ipi_dispatch_line" ]] \
    || ((ipi_decode_line <= ipi_clear_line)) \
    || ((ipi_dispatch_line <= ipi_decode_line)); then
    echo "LS2K1000 IPI handler must acknowledge before action dispatch" >&2
    exit 1
fi

# Linux wait queues register the waiter before the final condition check. Keep
# the timeout variant in the same order so an IRQ notification cannot be lost
# between the first check and listener registration.
wait_timeout_impl=$(
    sed -n '/pub fn wait_timeout_until/,/pub fn notify_one/p' \
        "$repo_root/vendor/axtask/src/wait_queue.rs"
)
mapfile -t wait_condition_lines < <(
    rg -n 'if condition\(\)' <<<"$wait_timeout_impl" | cut -d: -f1
)
wait_listener_line=$(rg -n 'listener!\(self\.event' <<<"$wait_timeout_impl" | cut -d: -f1)
wait_await_line=$(rg -n 'timeout_at\(Some\(deadline\), listener\)' <<<"$wait_timeout_impl" | cut -d: -f1)
if ((${#wait_condition_lines[@]} < 2)) \
    || ((wait_condition_lines[1] <= wait_listener_line)) \
    || ((wait_condition_lines[1] >= wait_await_line)); then
    echo "wait_timeout_until must recheck its condition after listener registration" >&2
    exit 1
fi

# Per-return load balancing used to allocate a 4 KiB migration helper on a
# hot syscall/trap path.  A build workload can execute thousands of syscalls
# per second, so placement belongs at task creation or explicit affinity
# changes, not at every return to user mode.
if rg -q 'rebalance_current\(\)' "$repo_root/kernel/src/task/user.rs"; then
    echo "user return path must not spawn per-syscall rebalance helpers" >&2
    exit 1
fi

# Console signals are asynchronous device events, not a side effect of a
# foreground program calling read(2).  Register the IRQ waker before enabling
# the NS16550 receive interrupt, consume signal characters in all ISIG modes,
# and honor the process group supplied by TIOCSPGRP.
tty_reader_impl=$(<"$repo_root/kernel/src/pseudofs/dev/tty/ntty.rs")
tty_ldisc_impl=$(<"$repo_root/kernel/src/pseudofs/dev/tty/terminal/ldisc.rs")
tty_termios_impl=$(<"$repo_root/kernel/src/pseudofs/dev/tty/terminal/termios.rs")
tty_ioctl_impl=$(<"$repo_root/kernel/src/pseudofs/dev/tty/mod.rs")
process_signal_impl=$(<"$repo_root/vendor/starry-signal/src/api/process.rs")
kernel_signal_impl=$(<"$repo_root/kernel/src/task/signal.rs")
console_impl=$(<"$repo_root/vendor/axplat-loongarch64-ls2k1000/src/console.rs")
irq_waker_impl=$(<"$repo_root/vendor/axtask/src/future/poll.rs")
tty_register_line=$(rg -n 'register_irq_waker\(irq, &waker\)' <<<"$tty_reader_impl" | cut -d: -f1)
tty_enable_line=$(rg -n 'axhal::console::enable_rx_interrupt\(\)' <<<"$tty_reader_impl" | cut -d: -f1)
tty_signal_impl=$(
    sed -n '/fn check_send_signal/,/fn output_char/p' \
        "$repo_root/kernel/src/pseudofs/dev/tty/terminal/ldisc.rs"
)
if [[ -z "$tty_register_line" || -z "$tty_enable_line" ]] \
    || ((tty_enable_line <= tty_register_line)) \
    || ! rg -q 'UART_IER_RECEIVED_DATA_AVAILABLE' <<<"$console_impl" \
    || ! rg -q 'fn enable_rx_interrupt\(\)' <<<"$console_impl" \
    || ! rg -q 'let changed = \{' <<<"$console_impl" \
    || ! rg -q 'if changed \{' <<<"$console_impl" \
    || ! rg -q 'axhal::irq::set_enable\(irq, false\)' <<<"$irq_waker_impl" \
    || ! rg -q 'first_registration' <<<"$irq_waker_impl" \
    || ! rg -q 'axhal::irq::register\(irq, irq_event_marker\)' <<<"$irq_waker_impl" \
    || ! rg -q 'axhal::irq::set_enable\(irq, true\)' <<<"$irq_waker_impl" \
    || rg -q 'term\.canonical\(\)' <<<"$tty_signal_impl" \
    || ! rg -q 'term\.has_lflag\(ISIG\)' <<<"$tty_signal_impl" \
    || ! rg -q 'term\.has_lflag\(NOFLSH\)' <<<"$tty_signal_impl" \
    || ! rg -q 'MOSS_TTY_SIGNAL' <<<"$tty_signal_impl" \
    || ! rg -q 'pub fn send_signal\(&self, sig: SignalInfo\) -> Vec<u32>' \
        <<<"$process_signal_impl" \
    || ! rg -q 'result\.push\(\*tid\)' <<<"$process_signal_impl" \
    || ! rg -q 'MOSS_TTY_SIGNAL_DELIVERY' <<<"$kernel_signal_impl" \
    || ! rg -q 'continue;' <<<"$tty_ldisc_impl" \
    || ! rg -q '\(VSUSP, Signo::SIGTSTP\)' <<<"$tty_termios_impl" \
    || ! rg -q 'let pgid = \(arg as \*const u32\)\.vm_read\(\)' <<<"$tty_ioctl_impl" \
    || ! rg -q 'get_process_group\(pgid\)' <<<"$tty_ioctl_impl" \
    || rg -q 'set_foreground\(&curr\.as_thread' <<<"$tty_ioctl_impl"; then
    echo "console IRQ, line discipline, and foreground process-group signals must stay connected" >&2
    exit 1
fi

# The competition workload is invoked directly. The kernel watchdog adds a
# twenty-minute sliding no-progress lease: real user-task execution or explicit
# boot/shutdown milestones renew it, but mere syscall volume does not.
init_final=$(<"$repo_root/src/init_final.sh")
watchdog_impl=$(<"$repo_root/kernel/src/watchdog.rs")
runtime_impl=$(<"$repo_root/vendor/axruntime/src/lib.rs")
axtask_api_impl=$(<"$repo_root/vendor/axtask/src/api.rs")
if rg -q 'BUILDSTORM_TIMEOUT_SECONDS|MOSS_BUILDSTORM_(LIMIT|TIMEOUT)' \
        <<<"$init_final" \
    || ! rg -q 'bash /glibc/buildstorm_testcode\.sh' <<<"$init_final"; then
    echo "buildstorm must remain an unmodified direct call to the competition workload" >&2
    exit 1
fi
if rg -q 'moss-build-cache|MOSS_(RUSTC|PROJECT|BUILD_CACHE)_TMPFS|mount -t tmpfs|mount --bind' \
        <<<"$init_final"; then
    echo "the 1 GiB target must not duplicate rustc or the build tree in tmpfs" >&2
    exit 1
fi
if rg -q 'RUN_DEADLINE_SECS|watchdog run deadline reached' <<<"$watchdog_impl" \
    || ! rg -q 'RUNTIME_TIMEOUT_SECS: u32 = 30' <<<"$watchdog_impl" \
    || ! rg -q 'PROGRESS_LEASE_SECS: u64 = 20 \* 60' <<<"$watchdog_impl" \
    || ! rg -q 'BUILD_TASK_SNAPSHOT_INTERVAL_SECS: u64 = 5 \* 60' <<<"$watchdog_impl" \
    || ! rg -q 'LS2K1000 build task:' <<<"$watchdog_impl" \
    || ! rg -q 'elapsed_ns = now_ns\.saturating_sub\(started_ns\)' \
        <<<"$watchdog_impl" \
    || ! rg -q 'user_identity_changed\(user_progress, current_user_progress\)' \
        <<<"$watchdog_impl" \
    || ! rg -q 'CPU_WORK_PROGRESS_PERMILLE_ONE_CORE: u64 = 100' \
        <<<"$watchdog_impl" \
    || ! rg -q 'cpu_idle_time_nanos' <<<"$watchdog_impl" \
    || ! rg -q 'cpu_user_task_time_nanos' <<<"$watchdog_impl" \
    || ! rg -q 'fn sample_user_cpu_work' <<<"$watchdog_impl" \
    || rg -q 'fn sample_cpu_work' <<<"$watchdog_impl" \
    || ! rg -q 'static CPU_USER_TASK_NANOS' <<<"$axtask_api_impl" \
    || ! rg -q 'fn account_user_task_time' <<<"$axtask_api_impl" \
    || ! rg -q 'task\.task_ext\(\)\.is_some\(\)' <<<"$axtask_api_impl" \
    || rg -q 'current_user_progress\.schedule_count != user_progress\.schedule_count' \
        <<<"$watchdog_impl" \
    || ! rg -q 'last_progress_ns = now_ns' <<<"$watchdog_impl" \
    || ! rg -q 'LS2K1000 memory:' <<<"$watchdog_impl" \
    || ! rg -q 'UsageKind::VirtMem' <<<"$watchdog_impl" \
    || ! rg -q 'UsageKind::PageCache' <<<"$watchdog_impl" \
    || ! rg -q 'watchdog progress lease expired' <<<"$watchdog_impl" \
    || ! rg -q 'withholding ping' <<<"$watchdog_impl" \
    || ! rg -q 'orphaned cooperative mutex' <<<"$watchdog_impl" \
    || ! rg -q 'axhal::watchdog::ping\(\);' <<<"$watchdog_impl"; then
    echo "LS2K1000 watchdog must slide on real progress and reset after twenty idle minutes" >&2
    exit 1
fi
if ! rg -q 'fn boot_watchdog_milestone' <<<"$runtime_impl" \
    || ! rg -q 'axhal::watchdog::ping\(\)' <<<"$runtime_impl" \
    || ! rg -q 'MOSS_BOOT_STAGE \{stage\}' <<<"$runtime_impl" \
    || ! rg -q 'boot_watchdog_milestone\("paging-ready"\)' <<<"$runtime_impl" \
    || ! rg -q 'boot_watchdog_milestone\("devices-ready"\)' <<<"$runtime_impl" \
    || ! rg -q 'boot_watchdog_milestone\("main-entry"\)' <<<"$runtime_impl"; then
    echo "early boot must reload the short watchdog at explicit subsystem milestones" >&2
    exit 1
fi

# MOSS reuses only the physical data blocks of sda1's swapfile. Linux must
# resolve and validate the live ext4 extent map before handoff; page zero stays
# reserved for the Linux swap header. U-Boot accepts at most 32 arguments, so
# the runner splits the handoff into bounded properties and arms the hardware
# watchdog before those FDT commands.
swap_impl=$(<"$repo_root/kernel/src/swap.rs")
swap_runner=$(<"$repo_root/scripts/run_ls2k1000_serial_tftp.py")
cow_impl=$(<"$repo_root/kernel/src/mm/aspace/backend/cow.rs")
loongarch_pte_impl=$(<"$repo_root/vendor/page_table_entry/src/arch/loongarch64.rs")
page_table_impl=$(<"$repo_root/vendor/page_table_multiarch/src/bits64.rs")
if ! rg -q 'SWAP_HEADER_PAGES: usize = 1' <<<"$swap_impl" \
    || ! rg -q 'moss,swap-extents-0' <<<"$swap_impl" \
    || ! rg -q 'ls2k1000_raw_write' <<<"$swap_impl" \
    || ! rg -q 'ls2k1000_raw_read' <<<"$swap_impl" \
    || ! rg -q 'filefrag -b4096 -v /swapfile-moss' <<<"$swap_runner" \
    || ! rg -q 'range\(0, len\(swap_extents\), 8\)' <<<"$swap_runner" \
    || ! rg -q 'moss,swap-extents-\{index\}' <<<"$swap_runner" \
    || ! rg -q 'signal\.signal\(signal\.SIGINT, forward_sigint\)' <<<"$swap_runner" \
    || ! rg -q 'self\.write\(b"\\x03"\)' <<<"$swap_runner" \
    || ! rg -q 'pt\.finish\(\)' <<<"$cow_impl" \
    || ! rg -q 'frame_refs\(\)\.count\(paddr\) != Some\(1\)' <<<"$cow_impl" \
    || ! rg -q 'WORKING_SET_HOT_NANOS: u64 = 120' <<<"$cow_impl" \
    || ! rg -q 'WORKING_SET_PROBE_NANOS: u64 = 5' <<<"$cow_impl" \
    || ! rg -q 'pt\.clear_dirty\(vaddr\)' <<<"$cow_impl" \
    || ! rg -q 'PTEFlags::W \| PTEFlags::D' <<<"$loongarch_pte_impl" \
    || ! rg -q 'fn clear_dirty\(&mut self\) -> Option<bool>' <<<"$loongarch_pte_impl" \
    || ! rg -q 'pub fn clear_dirty.*PagingResult<Option<bool>>' <<<"$page_table_impl" \
    || ! rg -q 'mark_active\(plan\.vaddr\)' <<<"$cow_impl" \
    || ! rg -q 'mark_refaulted\(plan\.vaddr\)' <<<"$cow_impl" \
    || ! rg -q 'self\.extend_until\(vaddr, u64::MAX\)' <<<"$cow_impl" \
    || ! rg -q 'note_working_set_probes' <<<"$cow_impl" \
    || ! rg -q 'note_working_set_refault' <<<"$cow_impl" \
    || ! rg -q '!allow_recent && self\.recent_pages\.is_recent' <<<"$cow_impl" \
    || ! rg -q 'swapped COW page lost its slot ownership' <<<"$cow_impl" \
    || ! rg -q 'MAX_RECLAIM_PAGES: usize = 8192' <<<"$swap_impl" \
    || ! rg -q 'EMERGENCY_FREE_BYTES: usize = 64' <<<"$swap_impl" \
    || ! rg -q 'BACKGROUND_RECLAIM_PAGES: usize = 2048' <<<"$swap_impl" \
    || ! rg -q 'fn background_reclaim_worker\(\) -> !' <<<"$swap_impl" \
    || ! rg -q 'RECLAIM_WORK\.wait_until' <<<"$swap_impl" \
    || ! rg -q 'queue_background_reclaim\(aspace\)' <<<"$swap_impl" \
    || ! rg -q 'Some\(0\) => axtask::sleep\(BACKGROUND_PROBE_RETRY\)' <<<"$swap_impl" \
    || ! rg -q 'reclaim_current_inner\(limit, true\)' <<<"$swap_impl" \
    || ! rg -q 'static RECLAIM_WAIT: axtask::WaitQueue' <<<"$swap_impl" \
    || ! rg -q 'wait_until\(\|\| RECLAIM_ACTIVE' <<<"$swap_impl"; then
    echo "LS2K1000 swap must preserve shared/header pages and use bounded working-set reclaim" >&2
    exit 1
fi
uboot_watchdog_line=$(rg -n '^ *\("mw\.l 0x1fe27030 0", 5\)' <<<"$swap_runner" | head -n1 | cut -d: -f1)
uboot_fdt_line=$(rg -n '^ *\("fdt resize 4096", 5\)' <<<"$swap_runner" | head -n1 | cut -d: -f1)
if [[ -z "$uboot_watchdog_line" || -z "$uboot_fdt_line" ]] \
    || ((uboot_watchdog_line >= uboot_fdt_line)); then
    echo "U-Boot watchdog must cover the dynamic swap extent handoff" >&2
    exit 1
fi

platform_config=$(<"$repo_root/vendor/axplat-loongarch64-ls2k1000/axconfig.toml")
if ! rg -q 'phys-memory-size = 0x4000_0000' <<<"$platform_config" \
    || rg -q 'phys-memory-size = 0x8000_0000' <<<"$platform_config"; then
    echo "LS2K1000LA 1 GiB platform memory capacity must not be overstated as 2 GiB" >&2
    exit 1
fi

# LS2K1000LA RAM is split between a low bank and a high bank, with the kernel
# image cutting the latter into two more free ranges. Every range must remain
# page allocatable. Adding secondary banks directly to the byte heap leaves
# user address spaces with only the largest (about 632 MiB) continuous range.
bitmap_allocator_impl=$(<"$repo_root/vendor/axallocator/src/bitmap.rs")
global_allocator_impl=$(<"$repo_root/vendor/axalloc/src/lib.rs")
runtime_allocator_impl=$(
    sed -n '/fn init_allocator()/,/^}/p' "$repo_root/vendor/axruntime/src/lib.rs"
)
if ! rg -q 'fn add_memory\(&mut self, start: usize, size: usize\)' \
        <<<"$bitmap_allocator_impl" \
    || ! rg -q 'self\.inner\.insert\(start_idx\.\.end_idx\)' \
        <<<"$bitmap_allocator_impl" \
    || ! rg -q 'self\.total_pages \+= end_idx - start_idx' \
        <<<"$bitmap_allocator_impl" \
    || ! rg -q 'self\.palloc\.lock\(\)\.add_memory\(start_vaddr, size\)' \
        <<<"$global_allocator_impl" \
    || ! rg -q 'MAX_HEAP_EXPAND_SIZE: usize = 16 \* 1024 \* 1024' \
        <<<"$global_allocator_impl" \
    || ! rg -q '\.min\(MAX_HEAP_EXPAND_SIZE\)' <<<"$global_allocator_impl" \
    || ! rg -q '\.min_by_key\(\|r\| r\.paddr\)' <<<"$runtime_allocator_impl" \
    || ! rg -q 'global_add_memory\(phys_to_virt\(r\.paddr\).*r\.size\)' \
        <<<"$runtime_allocator_impl" \
    || ! rg -q 'MOSS_MEMORY managed_mib=' <<<"$runtime_allocator_impl"; then
    echo "all discontiguous LS2K1000LA RAM ranges must feed the page allocator" >&2
    exit 1
fi

# rustc maps its large shared libraries MAP_PRIVATE. Keep the demanded page
# ahead of speculation, then pipeline a bounded 128 KiB window after the
# second sequential COW fault. The file-cache watermarks remain responsible
# for reclaim; no tmpfs duplicate or permanently pinned fault-around window.
cow_impl=$(<"$repo_root/kernel/src/mm/aspace/backend/cow.rs")
if ! rg -q 'COW_MMAP_READAHEAD_PAGES: u64 = 32' <<<"$cow_impl" \
    || ! rg -q 'previous\.checked_add\(1\) != Some\(fault_page\)' <<<"$cow_impl" \
    || ! rg -q 'after_fault\(file, fault_file_start, \*file_end\)' <<<"$cow_impl" \
    || ! rg -q 'file\.readahead\(start \* page_size, count\)' <<<"$cow_impl"; then
    echo "MAP_PRIVATE file faults must retain bounded sequential readahead" >&2
    exit 1
fi

allocation_impl=$(<"$repo_root/kernel/src/mm/aspace/backend/mod.rs")
file_cache_impl=$(<"$repo_root/vendor/axfs/src/highlevel/file.rs")
pressure_reclaim_impl=$(
    sed -n '/fn reclaim_pages/,/fn reclaim_page_cache_if_needed/p' \
        "$repo_root/vendor/axfs/src/highlevel/file.rs"
)
if ! rg -q 'reclaim_global_file_cache_for_memory_pressure\(\)' <<<"$allocation_impl" \
    || ! rg -q 'pub fn reclaim_global_file_cache_for_memory_pressure\(\)' \
        <<<"$file_cache_impl" \
    || ! rg -q 'PAGE_CACHE_RECLAIM_LOCK\.lock\(\)' <<<"$file_cache_impl" \
    || rg -q 'PAGE_CACHE_RECLAIM_LOCK\.try_lock\(\)' <<<"$file_cache_impl" \
    || ! rg -q 'reclaim_page_caches_to\(target, None\)' <<<"$file_cache_impl" \
    || ! rg -q 'PRESSURE_RECLAIM_RETRY_PAGES: usize = 1024' <<<"$file_cache_impl" \
    || ! rg -q 'LARGE_FOLIO_RETRY_NANOS: u64 = 30 \* 1_000_000_000' \
        <<<"$file_cache_impl" \
    || ! rg -q 'large_folio_allocation_failures' <<<"$file_cache_impl" \
    || rg -q 'Failed to allocate order-\{order\} folio' <<<"$file_cache_impl" \
    || ! rg -q 'fn reclaim_pages\(&self, limit: usize\) -> usize' \
        <<<"$pressure_reclaim_impl" \
    || rg -q 'writeback_folio|write_fully_at|commit_len|wait_uptodate' \
        <<<"$pressure_reclaim_impl" \
    || ! rg -q 'Speculative I/O must yield its memory budget' \
        <<<"$file_cache_impl"; then
    echo "anonymous faults must reclaim clean file cache at the memory watermark" >&2
    exit 1
fi

# Initial SATA discovery must enter the same Linux-style EH reset path as
# runtime recovery. A fixed one-second DET/TFD gate here drops slow disks
# before debounce and reset retry logic can run.
ahci_completion_impl=$(<"$repo_root/vendor/simple-ahci/src/completion.rs")
ahci_command_impl=$(<"$repo_root/vendor/simple-ahci/src/ahci.rs")
if ! rg -q 'pub\(crate\) struct AdmissionTicket\(u64\)' <<<"$ahci_completion_impl" \
    || ! rg -q 'next: AtomicU64' <<<"$ahci_completion_impl" \
    || ! rg -q 'serving: AtomicU64' <<<"$ahci_completion_impl" \
    || ! rg -q 'self\.admission\.is_turn\(admission\)' <<<"$ahci_command_impl" \
    || ! rg -q 'release_admission\(admission\)' <<<"$ahci_command_impl" \
    || ! rg -q 'port: Box<AhciPort<H>>' <<<"$ahci_command_impl" \
    || ! rg -q 'candidate\.reinitialize_unpublished\(\)' <<<"$ahci_command_impl" \
    || ! rg -q 'self\.completion\.reinitialize_unpublished\(\)' <<<"$ahci_command_impl" \
    || rg -q 'admission\.try_acquire\(\)' <<<"$ahci_command_impl"; then
    echo "AHCI port must be stably allocated and use FIFO non-NCQ admission" >&2
    exit 1
fi

ahci_try_new_impl=$(
    sed -n '/fn try_new(/,/fn enable_interrupts/p' \
        "$repo_root/vendor/simple-ahci/src/ahci.rs"
)
if rg -q 'SSTS\(\)\.read\(\)\.DET\(\) == 3' <<<"$ahci_try_new_impl" \
    || ! rg -q 'recovery_needed: AtomicBool::new\(true\)' <<<"$ahci_try_new_impl" \
    || ! rg -q 'recover_if_needed\(WaitMode::Poll\)' <<<"$ahci_try_new_impl"; then
    echo "AHCI initial probe must use the shared debounce/reset recovery path" >&2
    exit 1
fi

# AHCI requires the command engine to stop before FIS reception is disabled.
# Clearing PxCMD.ST and PxCMD.FRE together is explicitly forbidden by the
# controller state machine and can wedge warm-booted HBAs.
ahci_deinit_impl=$(
    sed -n '/fn deinit_port(/,/fn clear_pending/p' \
        "$repo_root/vendor/simple-ahci/src/ahci.rs"
)
clear_start_line=$(rg -n 'PortDeinitAction::ClearStart' <<<"$ahci_deinit_impl" | cut -d: -f1)
wait_command_line=$(rg -n 'PortDeinitAction::WaitCommandEngine' <<<"$ahci_deinit_impl" | cut -d: -f1)
clear_fis_line=$(rg -n 'PortDeinitAction::ClearFisReceive' <<<"$ahci_deinit_impl" | cut -d: -f1)
wait_fis_line=$(rg -n 'PortDeinitAction::WaitFisReceive' <<<"$ahci_deinit_impl" | cut -d: -f1)
if [[ -z "$clear_start_line" || -z "$wait_command_line" \
    || -z "$clear_fis_line" || -z "$wait_fis_line" ]] \
    || ((clear_start_line >= wait_command_line)) \
    || ((wait_command_line >= clear_fis_line)) \
    || ((clear_fis_line >= wait_fis_line)); then
    echo "AHCI port deinit must stop command engine before FIS reception" >&2
    exit 1
fi

# Recovery serializes only the transition that freezes command issue. Never
# carry the spin::Mutex guard into the potentially minute-long reset waits;
# runtime waits may sleep and another task spinning on that guard can starve
# the recovering task on the same CPU.
ahci_recovery_impl=$(sed -n '/fn recover_if_needed/,/^struct AhciInner/p' \
    "$repo_root/vendor/simple-ahci/src/ahci.rs")
recovery_freeze_line=$(rg -n 'CMD\(\)\.update\(\|cmd\| cmd\.with_ST\(false\)\)' \
    <<<"$ahci_recovery_impl" | cut -d: -f1 || true)
recovery_unlock_line=$(rg -n 'drop\(issue_guard\)' \
    <<<"$ahci_recovery_impl" | cut -d: -f1 || true)
recovery_wait_line=$(rg -n 'wait_for_event_or_spin::<H, _>\(' \
    <<<"$ahci_recovery_impl" | tail -n 1 | cut -d: -f1 || true)
if [[ -z "$recovery_freeze_line" || -z "$recovery_unlock_line" \
    || -z "$recovery_wait_line" ]] \
    || ((recovery_unlock_line <= recovery_freeze_line)) \
    || ((recovery_unlock_line >= recovery_wait_line)) \
    || rg -q 'wait_until_timeout::<H>\(\|\| !self\.port\.CMD\(\)\.read\(\)\.CR\(\)' \
        <<<"$ahci_recovery_impl"; then
    echo "AHCI recovery must release its issue spinlock before sleep-capable waits" >&2
    exit 1
fi

# MOSS performs recovery synchronously in the caller rather than in Linux's
# independent libata EH worker. Keep that synchronous outage bounded to one
# five-second COMRESET; failed ports are retried later by the existing gate.
if ! rg -q '^const RESET_TIMEOUT_MS: u64 = 5_000;$' \
        "$repo_root/vendor/simple-ahci/src/ahci.rs" \
    || rg -q 'RESET_TIMEOUTS_MS|RESET_COOLDOWN_MS|35_000' \
        "$repo_root/vendor/simple-ahci/src/ahci.rs"; then
    echo "AHCI synchronous recovery must be one bounded five-second COMRESET" >&2
    exit 1
fi

# Runtime watchdog health must not be inferred from syscall duration: a long
# userspace compile and a task blocked in wait/read/accept are both healthy.
# Only a marked cooperative-mutex wait whose owner has exited or disappeared
# may suppress feeding; ordinary contention behind a live owner is healthy. A
# hard scheduler/IRQ stall suppresses feeding by preventing the monitor itself
# from running.
platform_init_impl=$(<"$repo_root/vendor/axplat-loongarch64-ls2k1000/src/init.rs")
serial_tftp_impl=$(<"$repo_root/scripts/run_ls2k1000_serial_tftp.py")
thread_impl=$(<"$repo_root/kernel/src/task/mod.rs")
syscall_impl=$(<"$repo_root/kernel/src/syscall/mod.rs")
task_impl=$(<"$repo_root/vendor/axtask/src/task.rs")
mutex_impl=$(<"$repo_root/vendor/axsync/src/mutex.rs")
if ! rg -q 'ORPHAN_MUTEX_GRACE_NS' <<<"$watchdog_impl" \
    || ! rg -q 'TaskState::Running \| TaskState::Ready' <<<"$watchdog_impl" \
    || ! rg -q 'mutex_wait_snapshot' <<<"$watchdog_impl" \
    || ! rg -q 'task_by_id' <<<"$watchdog_impl" \
    || ! rg -q 'owner.state\(\) == TaskState::Exited' <<<"$watchdog_impl" \
    || ! rg -q 'begin_mutex_wait' <<<"$task_impl" \
    || ! rg -q 'end_mutex_wait' <<<"$task_impl" \
    || ! rg -q 'current\(\)\.begin_mutex_wait' <<<"$mutex_impl" \
    || ! rg -q 'current\(\)\.end_mutex_wait' <<<"$mutex_impl" \
    || ! rg -q 'task_by_id\(owner_id\)' <<<"$mutex_impl" \
    || ! rg -q 'report_recovered_mutex' <<<"$mutex_impl" \
    || ! rg -q 'watchdog_enter_syscall' <<<"$thread_impl" \
    || ! rg -q 'watchdog_exit_syscall' <<<"$thread_impl" \
    || ! rg -q 'watchdog::enter_syscall' <<<"$syscall_impl" \
    || rg -q 'RUNNABLE_SYSCALL_GRACE_NS|LAST_PROGRESS_NS|WORKLOAD_STALE|note_syscall' \
        <<<"$watchdog_impl"; then
    echo "watchdog must gate on orphaned cooperative mutexes, not syscall duration or live-owner contention" >&2
    exit 1
fi
if ! rg -q 'watchdog::start\(crate::watchdog::BOOT_TIMEOUT_SECS\)' \
        <<<"$platform_init_impl" \
    || ! rg -q 'node_clock::boost_to_900mhz\(\)' <<<"$platform_init_impl" \
    || rg -q 'watchdog::stop\(\)' <<<"$platform_init_impl" \
    || ! rg -q 'mw\.l 0x1fe27038 0xdf847580' <<<"$serial_tftp_impl" \
    || ! rg -q 'mw\.l 0x1fe27030 2' <<<"$serial_tftp_impl" \
    || ! rg -q 'mw\.l 0x1fe27034 1' <<<"$serial_tftp_impl" \
    || ! rg -q 'parser\.add_argument\(' <<<"$serial_tftp_impl" \
    || ! rg -q '"--log"' <<<"$serial_tftp_impl" \
    || ! rg -q 'self\.log_file\.write\(data\)' <<<"$serial_tftp_impl"; then
    echo "LS2K1000 early boot must remain under the 30-second hardware watchdog" >&2
    exit 1
fi

# Use Loongson's published 900 MHz/1.05 V U-Boot node-PLL setting.  The
# transition is volatile and happens only after the early watchdog and console
# are available; it must complete before secondary CPUs are released.
node_clock_impl=$(<"$repo_root/vendor/axplat-loongarch64-ls2k1000/src/node_clock.rs")
clock_call_line=$(rg -n 'node_clock::boost_to_900mhz' <<<"$platform_init_impl" | cut -d: -f1)
watchdog_call_line=$(rg -n 'watchdog::start' <<<"$platform_init_impl" | head -n 1 | cut -d: -f1)
console_call_line=$(rg -n 'console::init_early' <<<"$platform_init_impl" | cut -d: -f1)
clock_select_line=$(rg -n 'value &= !PLL_SELECT' <<<"$node_clock_impl" | cut -d: -f1)
clock_power_down_line=$(rg -n 'value \|= PLL_POWER_DOWN' <<<"$node_clock_impl" | cut -d: -f1)
clock_soft_set_line=$(rg -n 'value \|= PLL_SOFT_SET' <<<"$node_clock_impl" | cut -d: -f1)
if [[ -z "$clock_call_line" || -z "$watchdog_call_line" || -z "$console_call_line" \
    || -z "$clock_select_line" || -z "$clock_power_down_line" || -z "$clock_soft_set_line" ]] \
    || ((clock_call_line <= watchdog_call_line)) \
    || ((clock_call_line <= console_call_line)) \
    || ((clock_power_down_line <= clock_select_line)) \
    || ((clock_soft_set_line <= clock_power_down_line)) \
    || ! rg -q 'write64\(NODE_PLL_HIGH_PADDR, TARGET_HIGH_CONFIG\)' <<<"$node_clock_impl" \
    || ! rg -q 'locked_low \| PLL_SELECT' <<<"$node_clock_impl"; then
    echo "LS2K1000 node clock must follow the official watchdog-guarded Linux runtime PLL sequence" >&2
    exit 1
fi

# A completed board run must remain available for the next automated TFTP
# cycle. QEMU and other platforms still use their normal power-off path.
entry_impl=$(<"$repo_root/kernel/src/entry.rs")
if ! rg -q 'Init flow completed, rebooting LS2K1000LA' <<<"$entry_impl" \
    || ! rg -q 'axhal::power::system_reset\(\)' <<<"$entry_impl" \
    || ! rg -q '#\[cfg\(not\(feature = "ls2k1000"\)\)\]' <<<"$entry_impl"; then
    echo "completed LS2K1000 runs must reboot while other platforms power off" >&2
    exit 1
fi

# A failed PLV0 unaligned emulation is a kernel fault, not a recoverable user
# event. Preserve the full faulting instruction and trap frame for offline
# symbolization instead of panicking inside Result::unwrap and losing them.
loongarch_trap_impl=$(<"$repo_root/vendor/axcpu/src/loongarch64/trap.rs")
if ! rg -q 'badi::read\(\)\.inst\(\)' <<<"$loongarch_trap_impl" \
    || ! rg -q 'Failed to emulate PLV0 unaligned access' <<<"$loongarch_trap_impl" \
    || ! rg -q 'Unhandled trap.*fault_vaddr' <<<"$loongarch_trap_impl" \
    || ! rg -q 'Trap::Unknown if estat\.ecode\(\) == 0 && estat\.is\(\) == 0' \
        <<<"$loongarch_trap_impl" \
    || ! rg -q 'estat\.ecode\(\)' <<<"$loongarch_trap_impl" \
    || rg -q 'emulate_unaligned\(\)\.unwrap\(\)' <<<"$loongarch_trap_impl"; then
    echo "PLV0 faults must retain BADV, BADI, ESTAT, and the complete trap frame" >&2
    exit 1
fi

# LA264 returns zero in BADI for ALE despite ERA retaining the faulting PC.
# User emulation must fall back to an ERA instruction fetch, and its bytewise
# load/store helpers must run inside the normal scoped user-memory access path
# so a lazy user page can be populated instead of hitting the exception-table
# failure return on the first byte.
loongarch_uspace_impl=$(<"$repo_root/vendor/axcpu/src/loongarch64/uspace.rs")
user_task_impl=$(<"$repo_root/kernel/src/task/user.rs")
if ! rg -q 'hardware_badi == 0' <<<"$loongarch_uspace_impl" \
    || ! rg -q 'read_volatile\(self\.era as \*const u32\)' <<<"$loongarch_uspace_impl" \
    || ! rg -q 'Trap::Unknown if estat\.ecode\(\) == 0 && estat\.is\(\) == 0' \
        <<<"$loongarch_uspace_impl" \
    || ! rg -U -q 'access_user_memory\(\|\| unsafe \{[[:space:]]*uctx\.emulate_unaligned_at' \
        <<<"$user_task_impl"; then
    echo "user ALE emulation must fetch zero BADI from ERA and allow lazy user-page faults" >&2
    exit 1
fi

# The pinned LoongArch toolchain has produced bad atomic state while moving
# both client and accepted TCP sockets as large aggregates. Allocate the outer
# state first, then initialize every field at its final address. LoongArch's
# cooperative mutex path must not retain the unused heap-backed Event fields.
accept_impl=$(<"$repo_root/kernel/src/syscall/net/socket.rs")
tcp_impl=$(<"$repo_root/vendor/axnet/src/tcp.rs")
local_tcp_impl=$(<"$repo_root/vendor/axnet/src/local.rs")
axsync_impl=$(<"$repo_root/vendor/axsync/src/mutex.rs")
if ! rg -q 'Arc::new\(Socket\(listener\.accept\(\)\?\)\)' <<<"$accept_impl" \
    || ! rg -q 'Arc::get_mut\(&mut socket\)' <<<"$accept_impl" \
    || ! rg -q 'reinitialize_accepted_after_move' <<<"$accept_impl" \
    || ! rg -q 'pub struct TcpSocket\(Box<TcpSocketInner>\)' <<<"$tcp_impl" \
    || ! rg -q 'Box::<TcpSocketInner>::new_uninit\(\)' <<<"$tcp_impl" \
    || ! rg -q 'storage\.assume_init\(\)' <<<"$tcp_impl" \
    || ! rg -q 'StateLock::write_unpublished_in_place' <<<"$tcp_impl" \
    || ! rg -q 'LocalTcpSocket::write_unpublished_in_place' <<<"$tcp_impl" \
    || ! rg -q 'GeneralOptions::write_unpublished_in_place' <<<"$tcp_impl" \
    || ! rg -q 'PollSet::write_unpublished_in_place' <<<"$tcp_impl" \
    || ! rg -q 'reinitialize_accepted_after_move' <<<"$tcp_impl" \
    || ! rg -q 'axsync::write_unpublished_in_place' <<<"$local_tcp_impl" \
    || ! rg -q 'reinitialize_unpublished\(&mut self\.stream\)' <<<"$local_tcp_impl" \
    || ! rg -q 'ptr::write\(self\.listener\.get_mut\(\), None\)' <<<"$local_tcp_impl" \
    || ! rg -q 'core::ptr::write\(raw, RawMutex::new\(\)\)' <<<"$axsync_impl" \
    || ! rg -U -q '#\[cfg\(not\(target_arch = "loongarch64"\)\)\][[:space:]]*event: Event' \
        <<<"$axsync_impl" \
    || rg -q 'LOCAL_TCP_ACCEPT_LOCKS_REPORTED|local TCP accept mutexes' <<<"$local_tcp_impl"; then
    echo "TCP state must be initialized in place before client or accepted publication" >&2
    exit 1
fi

# Metadata misses and writeback may yield, while cache hits must remain local
# to one inode. Cold loads therefore use cooperative sharded gates and each
# inode owns a separate cooperative flush lock; no global transaction may
# serialize every compiler process.
metadata_impl=$(<"$repo_root/vendor/axfs-ng-vfs/src/node/mod.rs")
if ! rg -q 'INODE_METADATA_CACHE_SHARDS: usize = 64' <<<"$metadata_impl" \
    || ! rg -q 'INODE_METADATA_LOAD_LOCKS: \[SleepMutex<\(\)>; INODE_METADATA_CACHE_SHARDS\]' \
        <<<"$metadata_impl" \
    || ! rg -q 'let _load = inode_metadata_load_lock\(key\)\.lock\(\)' <<<"$metadata_impl" \
    || ! rg -q 'flush_lock: Arc<SleepMutex<\(\)>>' <<<"$metadata_impl" \
    || rg -q 'INODE_METADATA_CACHE_TRANSACTION' <<<"$metadata_impl"; then
    echo "metadata cache must use sharded cooperative load gates and per-inode flush locks" >&2
    exit 1
fi

# Linux saves HOST_PORTS_IMPL before HOST_RESET and restores it afterwards;
# LS2K1000 demonstrably clears PI during reset. Reversing this ordering makes
# the only SATA port disappear before the per-port recovery code can run.
ahci_prepare_impl=$(
    sed -n '/pub unsafe fn prepare/,/pub unsafe fn try_new/p' \
        "$repo_root/vendor/simple-ahci/src/ahci.rs"
)
host_config_line=$(rg -n 'HostInitialConfig::new' <<<"$ahci_prepare_impl" | cut -d: -f1)
host_reset_line=$(rg -n 'with_HR\(true\)' <<<"$ahci_prepare_impl" | cut -d: -f1)
port_restore_line=$(rg -n 'port_map_restore' <<<"$ahci_prepare_impl" | head -n 1 | cut -d: -f1)
port_write_line=$(rg -n 'host\.pi\(\)\.write' <<<"$ahci_prepare_impl" | cut -d: -f1)
if [[ -z "$host_config_line" || -z "$host_reset_line" \
    || -z "$port_restore_line" || -z "$port_write_line" ]] \
    || ((host_config_line >= host_reset_line)) \
    || ((port_restore_line <= host_reset_line)) \
    || ((port_write_line <= host_reset_line)); then
    echo "AHCI must save PI before HBA reset and restore it afterwards" >&2
    exit 1
fi

# Platform AHCI follows Linux libahci_platform: firmware supplies the PI mask
# when the embedded controller's HOST_PORTS_IMPL register was not programmed.
# Keep the property value flowing through the API instead of substituting a
# board-specific constant in the low-level driver.
ls2k_ahci_probe_impl=$(
    sed -n '/impl DriverProbe for Ls2kAhciDriver/,/pub struct GmacHalImpl/p' \
        "$repo_root/vendor/axdriver/src/ls2k1000.rs"
)
dt_port_map_line=$(rg -n 'find_property\("ports-implemented"\)' \
    <<<"$ls2k_ahci_probe_impl" | cut -d: -f1)
port_map_call_line=$(rg -n 'prepare_with_port_map\(uncached_addr\(paddr\), platform_port_map\)' \
    <<<"$ls2k_ahci_probe_impl" | cut -d: -f1)
partition_probe_line=$(rg -n 'select_partition\(&inner\)' \
    <<<"$ls2k_ahci_probe_impl" | cut -d: -f1)
runtime_wait_line=$(rg -n 'inner\.enable_runtime_waits\(\)' \
    <<<"$ls2k_ahci_probe_impl" | cut -d: -f1)
if [[ -z "$dt_port_map_line" || -z "$port_map_call_line" \
    || -z "$partition_probe_line" || -z "$runtime_wait_line" ]] \
    || ((dt_port_map_line >= port_map_call_line)) \
    || ((runtime_wait_line <= partition_probe_line)) \
    || rg -q 'AHCI_PROBE_ACTIVE|AhciProbeGuard' \
        "$repo_root/vendor/axdriver/src/ls2k1000.rs"; then
    echo "AHCI DT ports-implemented must flow into a polling-only platform probe" >&2
    exit 1
fi

# block_on is the final scheduling boundary. Even a buggy caller must not park
# the scheduler-owned idle task; keep this protection ahead of run-queue entry.
block_on_impl=$(
    sed -n '/pub fn block_on/,/Error returned by/p' \
        "$repo_root/vendor/axtask/src/future/mod.rs"
)
idle_poll_line=$(rg -n 'if !may_park' <<<"$block_on_impl" | cut -d: -f1)
run_queue_line=$(rg -n 'let mut rq = current_run_queue' <<<"$block_on_impl" | cut -d: -f1)
if [[ -z "$idle_poll_line" || -z "$run_queue_line" ]] \
    || ((idle_poll_line >= run_queue_line)); then
    echo "axtask block_on must poll instead of parking the idle task" >&2
    exit 1
fi

# NetBufPtr is an owning raw pointer. Convert it back to NetBuf before any
# fallible transmit gate so link-down/reset errors return the buffer to its
# pool instead of leaking one entry per attempted packet.
dwmac_transmit_impl=$(
    sed -n '/fn transmit_one/,/fn reclaim_tx/p' \
        "$repo_root/vendor/axdriver_net/src/dwmac_ls2k1000.rs"
)
tx_ownership_line=$(rg -n 'NetBuf::from_buf_ptr' <<<"$dwmac_transmit_impl" | cut -d: -f1)
tx_start_line=$(rg -n 'self\.ensure_started\(\)' <<<"$dwmac_transmit_impl" | cut -d: -f1)
tx_link_line=$(rg -n '!self\.link_state\.is_up\(\)' <<<"$dwmac_transmit_impl" | cut -d: -f1)
if [[ -z "$tx_ownership_line" || -z "$tx_start_line" || -z "$tx_link_line" ]] \
    || ((tx_ownership_line >= tx_start_line)) \
    || ((tx_ownership_line >= tx_link_line)); then
    echo "DWMAC transmit must recover NetBuf ownership before fallible gates" >&2
    exit 1
fi

# A fatal IRQ that lands between PxCI and issued-bitmap publication must leave
# recovery_needed visible before PxIS is acknowledged or the active bitmap is
# sampled. Otherwise the submitter can observe cleared status and report a
# failed command as a fast success.
ahci_irq_impl=$(
    sed -n '/fn handle_interrupt(&self)/,/fn debounce_link/p' \
        "$repo_root/vendor/simple-ahci/src/ahci.rs"
)
fatal_recovery_line=$(rg -n 'self\.recovery_needed\.store\(true' <<<"$ahci_irq_impl" | cut -d: -f1)
port_ack_line=$(rg -n 'self\.port\.IS\(\)\.write' <<<"$ahci_irq_impl" | head -n 1 | cut -d: -f1)
active_snapshot_line=$(rg -n 'let active = self\.completion\.issued' <<<"$ahci_irq_impl" | cut -d: -f1)
if [[ -z "$fatal_recovery_line" || -z "$port_ack_line" || -z "$active_snapshot_line" ]] \
    || ((fatal_recovery_line >= port_ack_line)) \
    || ((fatal_recovery_line >= active_snapshot_line)); then
    echo "AHCI fatal IRQ must publish recovery before ack and active-slot sampling" >&2
    exit 1
fi

state_tests=(
    "vendor/axplat-loongarch64-ls2k1000/src/boot_arg.rs:ls2k1000-boot-arg-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/node_clock_state.rs:ls2k1000-node-clock-tests"
    "vendor/fdt-parser/src/lib.rs:fdt-parser-tests"
    "vendor/axhal/src/ram_source.rs:ram-source-tests"
    "vendor/axplat-loongarch64-ls2k1000/src/mem_layout.rs:ls2k1000-memory-layout-tests"
    "vendor/simple-ahci/src/hal.rs:ahci-wait-policy-tests"
    "vendor/simple-ahci/src/host_reset.rs:ahci-host-reset-tests"
    "vendor/simple-ahci/src/port_state.rs:ahci-port-state-tests"
    "vendor/simple-ahci/src/completion.rs:ahci-completion-tests"
    "vendor/axtask/src/block_context.rs:task-block-context-tests"
    "vendor/axdriver_block/src/block_range.rs:block-range-tests"
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

echo "[3/8] reject fixed LS2K1000 peripheral addresses in production sources"
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

echo "[4/8] build the two-core board image"
# The physical 115200-baud console is synchronous.  Build and process-heavy
# workloads trigger thousands of task/syscall info records, which can turn the
# console lock into part of the scheduler's hot path.  Keep the production
# board target at warn; dedicated diagnostic targets may still opt into info.
production_board_rule=$(
    sed -n '/^kernel-la-2k1000:/,/^[[:alnum:]_.-]\+:/p' \
        "$repo_root/Makefile"
)
if ! rg -q 'LOG=warn' <<<"$production_board_rule" \
    || rg -q 'LOG=info' <<<"$production_board_rule" \
    || ! rg -q 'MEM=1G' <<<"$production_board_rule" \
    || rg -q 'MEM=2G' <<<"$production_board_rule"; then
    echo "production LS2K1000LA image must use LOG=warn and the physical 1 GiB capacity" >&2
    exit 1
fi

# Registry-vendored crates are treated as immutable by Cargo, so source mtimes
# do not invalidate existing package fingerprints. Force every modified crate
# on the AHCI wait path out of the release cache; otherwise this check can
# relink stale scheduler or driver code while reporting a successful build.
RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-nightly-2025-05-20} \
    cargo clean \
        --manifest-path "$repo_root/Cargo.toml" \
        --config 'source.crates-io.replace-with="vendored-sources"' \
        --config "source.vendored-sources.directory='$repo_root/vendor'" \
        --package simple-ahci \
        --package axallocator \
        --package axalloc \
        --package axfs \
        --package axdriver_block \
        --package axdriver \
        --package lock_api \
        --package kernel_guard \
        --package kspin \
        --package axcpu \
        --package axsync \
        --package axnet \
        --package axtask \
        --package starry-kernel \
        --package axruntime \
        --package axplat-loongarch64-ls2k1000 \
        --release \
        --target loongarch64-unknown-none-softfloat \
        --target-dir "$repo_root/target"
make -C "$repo_root" kernel-la-2k1000

[[ -s "$image" ]] || {
    echo "missing board image: $image" >&2
    exit 1
}
[[ -s "$elf" ]] || {
    echo "missing board ELF: $elf" >&2
    exit 1
}

echo "[5/8] validate the boot header and ELF architecture"
[[ $(od -An -tx1 -N2 "$image" | tr -d '[:space:]') == 4d5a ]]
llvm-readelf -h "$elf" | rg -q 'Machine:[[:space:]]+LoongArch'

echo "[6/8] validate Linux-compatible LoongArch interrupt and switch context"
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

echo "[7/8] reject unaligned wide loads in the firmware DTB header path"
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

echo "[8/8] validate the uncached DMA section"
dma_section=$(
    llvm-readelf -SW "$elf" |
        awk '$2 == ".dma_uncached" || $3 == ".dma_uncached" { print; found = 1 } END { exit !found }'
)
[[ "$dma_section" == *" WA "* ]]
[[ "$dma_section" != *" AX "* ]]
[[ ${dma_section##* } == 4096 ]]

printf 'LS2K1000LA static checks passed: cpus=%s image=%s bytes\n' \
    "$enabled_cpus" "$(stat -c %s "$image")"
