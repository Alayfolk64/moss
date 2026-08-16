#!/usr/bin/env python3
"""Native toolchains and xv6 acceptance shared by physical MOSS targets."""

from __future__ import annotations

import base64
import time
from typing import Protocol

from ls2k1000_apk_mariadb import require_shell_command


XV6_COMMIT = "7d01506ee03394b6b1a161ccee69cd150ab471fc"
XV6_ARCHIVE = "xv6-riscv-7d01506.tar.gz"
XV6_SHA256 = "3678060565355c13c68c1916a3cda3e5839b3ed9f6580312bb634172433942f6"
TOOLCHAIN_BUNDLE = "toolchain-offline.tar"


class HardwareRunner(Protocol):
    def emit_line(self, message: str) -> None: ...

    def linux_status_command(
        self, line: str, timeout: float = 20
    ) -> tuple[int, bytes]: ...

    def shell_command(
        self, line: str, timeout: float = 20
    ) -> tuple[int, bytes]: ...


def _base64_text(text: str) -> str:
    return base64.b64encode(text.encode("ascii")).decode("ascii")


def stage_toolchain_inputs(runner: HardwareRunner) -> None:
    """Stage signed APKs and pinned xv6 using factory Linux networking."""
    rc, _ = runner.linux_status_command(
        "( if findmnt -rn -S /dev/sda3 >/dev/null; then umount /dev/sda3; fi; "
        "sync; e2fsck -fy /dev/sda3; fsck_rc=$?; "
        "if [ \"$fsck_rc\" -gt 1 ]; then exit \"$fsck_rc\"; fi; "
        "mkdir -p /mnt/moss-toolchain-stage; "
        "mount /dev/sda3 /mnt/moss-toolchain-stage; "
        "( set -e; "
        "mkdir -p /mnt/moss-toolchain-stage/root/moss-toolchain; "
        "ip link set eth0 up; "
        "ip address replace 192.168.88.2/24 dev eth0; "
        "i=0; while [ \"$i\" -lt 30 ] && "
        "[ \"$(cat /sys/class/net/eth0/carrier 2>/dev/null)\" != 1 ]; do "
        "sleep 1; i=$((i + 1)); done; "
        "test \"$(cat /sys/class/net/eth0/carrier)\" = 1; "
        "if grep -q '^P:g++$' /mnt/moss-toolchain-stage/lib/apk/db/installed "
        "&& grep -q '^P:clang19$' "
        "/mnt/moss-toolchain-stage/lib/apk/db/installed "
        "&& grep -q '^P:llvm19$' "
        "/mnt/moss-toolchain-stage/lib/apk/db/installed "
        "&& grep -q '^P:lld$' "
        "/mnt/moss-toolchain-stage/lib/apk/db/installed "
        "&& grep -q '^P:perl$' "
        "/mnt/moss-toolchain-stage/lib/apk/db/installed "
        "&& grep -q '^P:git$' "
        "/mnt/moss-toolchain-stage/lib/apk/db/installed "
        "&& grep -q '^P:rust$' "
        "/mnt/moss-toolchain-stage/lib/apk/db/installed "
        "&& grep -q '^P:cargo$' "
        "/mnt/moss-toolchain-stage/lib/apk/db/installed; then "
        "rm -rf /mnt/moss-toolchain-stage/var/cache/moss-toolchain; "
        "echo MOSS_TOOLCHAIN_APKS_ALREADY_INSTALLED; else "
        "rm -rf /mnt/moss-toolchain-stage/var/cache/moss-toolchain; "
        "mkdir -p /mnt/moss-toolchain-stage/var/cache/moss-toolchain; "
        "busybox wget -O /mnt/moss-toolchain-stage/var/cache/"
        f"{TOOLCHAIN_BUNDLE}.new http://192.168.88.1:18083/"
        f"{TOOLCHAIN_BUNDLE}; "
        "tar -tf /mnt/moss-toolchain-stage/var/cache/"
        f"{TOOLCHAIN_BUNDLE}.new >/dev/null; "
        "tar -xf /mnt/moss-toolchain-stage/var/cache/"
        f"{TOOLCHAIN_BUNDLE}.new "
        "-C /mnt/moss-toolchain-stage/var/cache/moss-toolchain; "
        "rm /mnt/moss-toolchain-stage/var/cache/"
        f"{TOOLCHAIN_BUNDLE}.new; "
        "test \"$(find /mnt/moss-toolchain-stage/var/cache/moss-toolchain "
        "-maxdepth 1 -name '*.apk' | wc -l)\" -ge 40; "
        "for package in g++ clang19 llvm19 lld perl curl git rust cargo; do "
        "find /mnt/moss-toolchain-stage/var/cache/moss-toolchain "
        "-maxdepth 1 -name \"$package-*.apk\" | grep -q .; done; "
        "fi; "
        "busybox wget -O /mnt/moss-toolchain-stage/root/moss-toolchain/"
        f"{XV6_ARCHIVE}.new http://192.168.88.1:18083/{XV6_ARCHIVE}; "
        "test \"$(sha256sum /mnt/moss-toolchain-stage/root/moss-toolchain/"
        f"{XV6_ARCHIVE}.new | cut -d' ' -f1)\" = {XV6_SHA256}; "
        "mv /mnt/moss-toolchain-stage/root/moss-toolchain/"
        f"{XV6_ARCHIVE}.new /mnt/moss-toolchain-stage/root/moss-toolchain/"
        f"{XV6_ARCHIVE} ); stage_rc=$?; "
        "sync; umount /mnt/moss-toolchain-stage; exit \"$stage_rc\" )",
        timeout=600,
    )
    if rc != 0:
        raise RuntimeError(
            f"factory Linux toolchain staging failed with status {rc}"
        )
    runner.emit_line(
        "[serial-run] PASS toolchain inputs and official xv6 "
        f"{XV6_COMMIT[:7]} staged on /dev/sda3"
    )


def run_toolchain_xv6_test(runner: HardwareRunner) -> None:
    """Install and execute the native and cross-compilation acceptance set."""
    require_shell_command(
        runner,
        "Alpine musl rootfs preflight",
        "test \"$(. /etc/os-release; printf %s \"$ID\")\" = alpine && "
        "ldd --version 2>&1 | grep -qi musl && "
        "test \"$(getconf LONG_BIT)\" = 64 && "
        "uname -a && cat /etc/alpine-release && "
        "(df -h || true) && (free || true)",
    )
    host_epoch = int(time.time())
    require_shell_command(
        runner,
        "wall clock",
        f"board_epoch=$(date +%s); "
        f"if [ \"$board_epoch\" -lt {host_epoch - 86400} ] || "
        f"[ \"$board_epoch\" -gt {host_epoch + 86400} ]; then "
        f"date -u -s @{host_epoch}; fi; date -u",
    )
    installed_rc, _ = runner.shell_command(
        "apk info -e g++ >/dev/null && apk info -e clang19 >/dev/null && "
        "apk info -e llvm19 >/dev/null && "
        "(apk info -e lld >/dev/null || apk info -e lld20 >/dev/null) && "
        "apk info -e perl >/dev/null && apk info -e git >/dev/null && "
        "apk info -e rust >/dev/null && apk info -e cargo >/dev/null",
        timeout=60,
    )
    if installed_rc == 0:
        runner.emit_line("[serial-run] PASS toolchain packages already installed")
    else:
        require_shell_command(
            runner,
            "staged toolchain package signatures",
            "test \"$(find /var/cache/moss-toolchain -maxdepth 1 "
            "-name '*.apk' | wc -l)\" -ge 40 && "
            "apk verify /var/cache/moss-toolchain/*.apk",
            timeout=300,
        )
        require_shell_command(
            runner,
            "offline Alpine toolchain installation",
            "apk --no-network add /var/cache/moss-toolchain/*.apk && "
            "rm -rf /var/cache/moss-toolchain && sync",
            timeout=1200,
        )
    require_shell_command(
        runner,
        "gcc, g++, make, Git, Rust, and LLVM versions",
        "export PATH=/usr/lib/llvm19/bin:$PATH && "
        "apk info -e g++ && apk info -e clang19 && apk info -e llvm19 && "
        "(apk info -e lld || apk info -e lld20) && "
        "apk info -e git && apk info -e rust && apk info -e cargo && "
        "for tool in gcc g++ make git rustc cargo clang llvm-as opt llc "
        "llvm-readelf llvm-nm llvm-objcopy llvm-objdump ld.lld; do "
        "command -v \"$tool\" >/dev/null || exit 127; done && "
        "gcc --version >/tmp/moss-gcc-version && head -n 1 /tmp/moss-gcc-version && "
        "g++ --version >/tmp/moss-gpp-version && head -n 1 /tmp/moss-gpp-version && "
        "make --version >/tmp/moss-make-version && head -n 1 /tmp/moss-make-version && "
        "git --version && "
        "rustc --version && cargo --version && "
        "clang --version >/tmp/moss-clang-version && "
        "head -n 1 /tmp/moss-clang-version && "
        "llvm-as --version >/tmp/moss-llvm-as-version && "
        "head -n 1 /tmp/moss-llvm-as-version && "
        "ld.lld --version",
        timeout=180,
    )

    cpp_source = """#include <algorithm>
#include <iostream>
#include <numeric>
#include <string>
#include <vector>
int main() {
  std::vector<int> values{30, 10, 20};
  std::sort(values.begin(), values.end());
  const int total = std::accumulate(values.begin(), values.end(), 0);
  std::vector<std::string> names{"gamma", "alpha", "beta"};
  std::sort(names.begin(), names.end());
  std::cout << "GPP_MUSL_OK " << values.size() << ':' << total << ':'
            << names[0] << ',' << names[1] << ',' << names[2] << '\\n';
  return 0;
}
"""
    require_shell_command(
        runner,
        "native g++ compile and execution",
        "mkdir -p /root/moss-toolchain/native && "
        f"echo {_base64_text(cpp_source)} | base64 -d "
        "> /root/moss-toolchain/native/gpp.cpp && "
        "g++ -std=c++20 -O2 -Wall -Wextra -Werror "
        "/root/moss-toolchain/native/gpp.cpp "
        "-o /root/moss-toolchain/native/gpp-musl && "
        "result=$(/root/moss-toolchain/native/gpp-musl) && "
        "printf '%s\\n' \"$result\" && "
        "test \"$result\" = 'GPP_MUSL_OK 3:60:alpha,beta,gamma'",
        timeout=300,
    )

    rust_source = """fn main() {
    let mut values = vec![30, 10, 20];
    values.sort();
    let total: i32 = values.iter().sum();
    println!("RUST_CARGO_MUSL_OK {}:{}", values.len(), total);
}
"""
    cargo_toml = """[package]
name = "moss-rust-acceptance"
version = "0.1.0"
edition = "2021"
"""
    require_shell_command(
        runner,
        "native Rust and Cargo build and execution",
        "rm -rf /root/moss-toolchain/native/rust && "
        "mkdir -p /root/moss-toolchain/native/rust/src && "
        f"echo {_base64_text(cargo_toml)} | base64 -d > "
        "/root/moss-toolchain/native/rust/Cargo.toml && "
        f"echo {_base64_text(rust_source)} | base64 -d > "
        "/root/moss-toolchain/native/rust/src/main.rs && "
        "cd /root/moss-toolchain/native/rust && "
        "cargo build --release --offline && "
        "result=$(target/release/moss-rust-acceptance) && "
        "printf '%s\\n' \"$result\" && "
        "test \"$result\" = 'RUST_CARGO_MUSL_OK 3:60'",
        timeout=900,
    )

    require_shell_command(
        runner,
        "Git repository lifecycle",
        "rm -rf /root/moss-toolchain/native/git && "
        "mkdir -p /root/moss-toolchain/native/git && "
        "cd /root/moss-toolchain/native/git && git init && "
        "git config user.name MOSS && git config user.email moss@example.invalid && "
        "printf 'alpha\\nbeta\\n' > values.txt && git add values.txt && "
        "git commit -m 'record deterministic values' && "
        "test \"$(git rev-list --count HEAD)\" = 1 && "
        "test \"$(git show HEAD:values.txt | wc -l)\" = 2 && "
        "test -z \"$(git status --porcelain)\"",
        timeout=180,
    )

    llvm_source = """#include <stdio.h>
int main(void) {
  int values[] = {1, 2, 3, 4};
  int total = 0;
  for (unsigned i = 0; i < sizeof(values) / sizeof(values[0]); ++i)
    total += values[i];
  printf("LLVM_MUSL_OK %d\\n", total);
  return total == 10 ? 0 : 1;
}
"""
    require_shell_command(
        runner,
        "native LLVM IR pipeline and execution",
        "export PATH=/usr/lib/llvm19/bin:$PATH && "
        f"echo {_base64_text(llvm_source)} | base64 -d "
        "> /root/moss-toolchain/native/llvm.c && "
        "cd /root/moss-toolchain/native && "
        "clang -fPIC -O2 -Wall -Wextra -Werror -S -emit-llvm "
        "llvm.c -o llvm.ll && "
        "llvm-as llvm.ll -o llvm.bc && "
        "opt -passes=verify llvm.bc -o llvm.opt.bc && "
        "llc -relocation-model=pic -filetype=obj llvm.opt.bc -o llvm.o && "
        "clang llvm.o -o llvm-musl && result=$(./llvm-musl) && "
        "printf '%s\\n' \"$result\" && "
        "test \"$result\" = 'LLVM_MUSL_OK 10' && "
        "case \"$(uname -m)\" in "
        "riscv64) machine=RISC-V ;; loongarch64) machine=LoongArch ;; "
        "*) exit 64 ;; esac && "
        "llvm-readelf -h llvm-musl | grep -E \"Machine:.*$machine\"",
        timeout=300,
    )

    require_shell_command(
        runner,
        "pinned xv6 source archive",
        "cd /root/moss-toolchain && "
        f"test \"$(sha256sum {XV6_ARCHIVE} | cut -d' ' -f1)\" "
        f"= {XV6_SHA256} && "
        "rm -rf xv6-build && mkdir xv6-build && "
        f"tar -xzf {XV6_ARCHIVE} -C xv6-build --strip-components=1 && "
        "test -f xv6-build/Makefile && test -f xv6-build/kernel/main.c",
        timeout=180,
    )
    require_shell_command(
        runner,
        "xv6 LLVM compatibility patch",
        "cd /root/moss-toolchain/xv6-build && "
        "grep -Fq -- '-ggdb -gdwarf-2' Makefile && "
        "grep -Fq -- 'LDFLAGS = -z max-page-size=4096' Makefile && "
        "grep -Fq -- 'OUTPUT_ARCH( \"riscv\" )' user/user.ld && "
        "sed -i 's/-ggdb -gdwarf-2/-g0 -Wno-gnu-designator/' Makefile && "
        "sed -i 's/LDFLAGS = -z max-page-size=4096/"
        "LDFLAGS = -z max-page-size=4096 --image-base=0/' Makefile && "
        "sed -i '1a ENTRY(main)' user/user.ld && "
        "sed -i '/^  \\.rodata : {/i\\  . = ALIGN(0x1000);' "
        "user/user.ld && "
        "grep -Fq -- '-g0 -Wno-gnu-designator' Makefile && "
        "grep -Fq -- '--image-base=0' Makefile && "
        "grep -Fq -- 'ENTRY(main)' user/user.ld && "
        "grep -Fq -- '. = ALIGN(0x1000);' user/user.ld",
        timeout=180,
    )
    require_shell_command(
        runner,
        "xv6 RISC-V build with LLVM",
        "export PATH=/usr/lib/llvm19/bin:$PATH && "
        "cd /root/moss-toolchain/xv6-build && "
        "make -j2 TOOLPREFIX=llvm- "
        "'CC=clang --target=riscv64-unknown-elf' LD=ld.lld "
        "OBJCOPY=llvm-objcopy OBJDUMP=llvm-objdump kernel/kernel fs.img",
        timeout=1200,
    )
    require_shell_command(
        runner,
        "xv6 RISC-V artifacts",
        "export PATH=/usr/lib/llvm19/bin:$PATH && "
        "cd /root/moss-toolchain/xv6-build && "
        "test -s kernel/kernel && test -s fs.img && test -s user/_sh && "
        "test \"$(stat -c %s fs.img)\" = 2048000 && "
        "llvm-readelf -h kernel/kernel | grep -E 'Machine:.*RISC-V' && "
        "llvm-readelf -h user/_sh | grep -E 'Machine:.*RISC-V' && "
        "llvm-nm kernel/kernel | grep -E '[[:space:]]_entry$' && "
        "sha256sum kernel/kernel fs.img user/_sh && sync",
        timeout=300,
    )
    runner.emit_line(
        "[serial-run] ALL GCC/G++/MAKE/GIT/RUST/LLVM/XV6 TESTS PASSED"
    )
