#!/usr/bin/env python3
"""Run the native LoongArch64 GDB acceptance on LS2K1000LA hardware."""

from __future__ import annotations

import argparse
from functools import partial
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import re
import sys
import threading
import time
from typing import BinaryIO

from gdb_native_workload import ensure_gdb_packages, run_gdb_native_test
from run_ls2k1000_serial_tftp import SerialRunner, discover_swap_extents


HOST_IP = "192.168.88.1"
BOARD_IP = "192.168.88.2"


class QuietHandler(SimpleHTTPRequestHandler):
    allowed_name = ""

    def log_message(self, format: str, *args: object) -> None:
        return

    def do_GET(self) -> None:
        if self.path != f"/{self.allowed_name}":
            self.send_error(404)
            return
        super().do_GET()


class GdbSerialRunner(SerialRunner):
    def linux_status_command(
        self, line: str, timeout: float = 20
    ) -> tuple[int, bytes]:
        self.emit_line(f"[ls2k-gdb] >>> Linux: {line}")
        command = (
            line
            + "; __ls2k_gdb_linux_rc=$?; "
            + "printf '\\n__LS2K_GDB_LINUX_RC_%s__\\n' "
            + '"$__ls2k_gdb_linux_rc"\r'
        )
        self.write(command.encode("ascii"))
        deadline = time.monotonic() + timeout
        tail = b""
        marker = re.compile(rb"__LS2K_GDB_LINUX_RC_(\d+)__")
        while time.monotonic() < deadline:
            tail = (tail + self.read_and_emit(4096))[-2_097_152:]
            match = marker.search(tail)
            if match is not None:
                return int(match.group(1)), tail
        raise TimeoutError(f"factory Linux command timed out: {line}")

    def shell_command(
        self, line: str, timeout: float = 20
    ) -> tuple[int, bytes]:
        self.emit_line(f"[ls2k-gdb] >>> MOSS shell: {line}")
        self.write(b"\x03\r")
        prompt_deadline = time.monotonic() + 0.5
        while time.monotonic() < prompt_deadline:
            chunk = self.read_and_emit(4096)
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
        command = (
            line
            + "; __ls2k_gdb_moss_rc=$?; "
            + "printf '\\n__LS2K_GDB_MOSS_RC_%s__\\n' "
            + '"$__ls2k_gdb_moss_rc"\r'
        )
        self.write(command.encode("ascii"))
        deadline = time.monotonic() + timeout + min(300.0, 5 + len(command) / 8)
        tail = b""
        marker = re.compile(rb"__LS2K_GDB_MOSS_RC_(\d+)__")
        while time.monotonic() < deadline:
            chunk = self.read_and_emit(4096)
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
            tail = (tail + chunk)[-2_097_152:]
            match = marker.search(tail)
            if match is not None:
                return int(match.group(1)), tail
        raise TimeoutError(f"MOSS shell command timed out: {line}")

    def wait_for_moss_shell(self, timeout: float = 240) -> None:
        deadline = time.monotonic() + timeout
        tail = b""
        while time.monotonic() < deadline:
            chunk = self.read_and_emit(4096)
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
            tail = (tail + chunk)[-16384:]
            if b"starry:~#" in tail:
                rc, output = self.shell_command(
                    "echo LS2K_GDB_SHELL_READY", timeout=30
                )
                if rc == 0 and b"LS2K_GDB_SHELL_READY" in output:
                    self.emit_line("[ls2k-gdb] PASS MOSS shell ready")
                    return
            if b"[root@LS-GD" in tail and b"#" in tail:
                raise RuntimeError("factory Linux returned before MOSS shell")
        raise TimeoutError("LS2K1000LA MOSS shell did not become ready")


def run_checked(
    runner: GdbSerialRunner,
    line: str,
    timeout: float = 20,
) -> bytes:
    output = runner.command(line, timeout)
    errors = (
        b"Unknown command",
        b"TFTP error",
        b"Retry count exceeded",
        b"File not found",
    )
    if any(error in output for error in errors):
        raise RuntimeError(f"U-Boot command failed: {line}")
    return output


def stage_bundle(
    runner: GdbSerialRunner,
    root_device: str,
    bundle_url: str,
    package_floor: int,
) -> None:
    mount_dir = "/mnt/moss-gdb-stage"
    command = (
        f"( if findmnt -rn -S {root_device} >/dev/null; then "
        f"umount {root_device}; fi; sync; e2fsck -fy {root_device}; "
        "fsck_rc=$?; if [ \"$fsck_rc\" -gt 1 ]; then exit \"$fsck_rc\"; fi; "
        f"mkdir -p {mount_dir}; mount {root_device} {mount_dir}; "
        "stage_rc=0; ( set -e; ip link set eth0 up; "
        f"ip address replace {BOARD_IP}/24 dev eth0; "
        "i=0; while [ \"$i\" -lt 30 ] && "
        "[ \"$(cat /sys/class/net/eth0/carrier 2>/dev/null)\" != 1 ]; do "
        "sleep 1; i=$((i + 1)); done; "
        "test \"$(cat /sys/class/net/eth0/carrier)\" = 1; "
        f"rm -rf {mount_dir}/var/cache/moss-gdb-native "
        f"{mount_dir}/var/cache/moss-gdb-native.tar.new; "
        f"mkdir -p {mount_dir}/var/cache/moss-gdb-native; "
        f"busybox wget -O {mount_dir}/var/cache/moss-gdb-native.tar.new "
        f"{bundle_url}; "
        f"tar -tf {mount_dir}/var/cache/moss-gdb-native.tar.new >/dev/null; "
        f"tar -xf {mount_dir}/var/cache/moss-gdb-native.tar.new "
        f"-C {mount_dir}/var/cache/moss-gdb-native; "
        f"rm {mount_dir}/var/cache/moss-gdb-native.tar.new; "
        f"test -f {mount_dir}/var/cache/moss-gdb-native/gdb-native-demo.c; "
        f"test -f {mount_dir}/var/cache/moss-gdb-native/gdb-native-smoke.gdb; "
        f"test \"$(find {mount_dir}/var/cache/moss-gdb-native -maxdepth 1 "
        f"-name '*.apk' | wc -l)\" -ge {package_floor} ) || stage_rc=$?; "
        f"sync; umount {mount_dir}; exit \"$stage_rc\" )"
    )
    rc, _ = runner.linux_status_command(command, timeout=600)
    if rc != 0:
        raise RuntimeError(f"factory Linux staging failed with status {rc}")
    runner.emit_line(
        "[ls2k-gdb] PASS offline GDB packages and demo staged on /dev/sda3"
    )


def verify_filesystem(
    runner: GdbSerialRunner, root_device: str
) -> None:
    rc, _ = runner.linux_status_command(
        "systemctl stop systemd-udevd-control.socket "
        "systemd-udevd-kernel.socket systemd-udevd.service "
        "2>/dev/null || true; "
        f"if findmnt -rn -S {root_device} >/dev/null; then "
        f"umount {root_device}; fi; sync; "
        f"! findmnt -rn -S {root_device} >/dev/null",
        timeout=45,
    )
    if rc != 0:
        raise RuntimeError(f"factory Linux could not unmount {root_device}")
    rc, output = runner.linux_status_command(
        f"e2fsck -fn {root_device}", timeout=300
    )
    errors = rb"wrong|UNEXPECTED INCONSISTENCY|FILE SYSTEM WAS MODIFIED|still has errors"
    if rc != 0 or re.search(errors, output, re.IGNORECASE):
        raise RuntimeError(
            f"e2fsck -fn {root_device} failed with exit status {rc}"
        )
    runner.emit_line(
        f"[ls2k-gdb] PASS {root_device} e2fsck read-only check"
    )


def boot_moss(
    runner: GdbSerialRunner,
    *,
    image_name: str,
    root_device: str,
    hard_deadline: int,
    state: dict[str, bool],
) -> None:
    rc, _ = runner.linux_status_command(
        "systemctl stop systemd-udevd-control.socket "
        "systemd-udevd-kernel.socket systemd-udevd.service "
        "2>/dev/null || true; "
        f"if findmnt -rn -S {root_device} >/dev/null; then "
        f"umount {root_device}; fi; sync; "
        f"! findmnt -rn -S {root_device} >/dev/null",
        timeout=45,
    )
    if rc != 0:
        raise RuntimeError(f"factory Linux could not release {root_device}")
    swap_extents = discover_swap_extents(runner)
    runner.emit_line("[ls2k-gdb] rebooting into guarded U-Boot handoff")
    runner.write(b"/sbin/reboot -f\r")
    runner.stop_uboot_autoboot()
    swap_commands = [
        "fdt set /chosen moss,swap-extents-{} <{}>".format(
            index,
            " ".join(
                f"0x{logical:x} 0x{lba:x} 0x{pages:x}"
                for logical, lba, pages in swap_extents[start : start + 8]
            ),
        )
        for index, start in enumerate(range(0, len(swap_extents), 8))
    ]
    watchdog_commands = [
        ("mw.l 0x1fe27030 0", 5),
        ("mw.l 0x1fe27038 0xdf847580", 5),
        ("mw.l 0x1fe27030 2", 5),
        ("mw.l 0x1fe27034 1", 5),
    ]
    for command, timeout in watchdog_commands:
        run_checked(runner, command, timeout)
    state["guarded"] = True
    state["factory_returned"] = False
    commands = [
        (f"setenv ipaddr {BOARD_IP}", 5),
        (f"setenv serverip {HOST_IP}", 5),
        ("setenv netmask 255.255.255.0", 5),
        ("setenv ethact ethernet@40040000", 5),
        ("tftpboot 0x900000000a000000 board.dtb", 30),
        ("fdt addr 0x900000000a000000", 5),
        ("fdt resize 4096", 5),
        (f"fdt set /chosen bootargs root={root_device}", 5),
        (
            "fdt set /chosen moss,watchdog-hard-deadline-secs "
            f"<{hard_deadline}>",
            5,
        ),
        *((command, 5) for command in swap_commands),
        ("fdt set /aliases ethernet0 /2k1000-soc/ethernet@40040000", 5),
        (
            "fdt set /2k1000-soc/ahci@400e0000 ports-implemented <0x1>",
            5,
        ),
        (f"tftpboot 0x9000000098000000 {image_name}", 45),
    ]
    for command, timeout in commands:
        run_checked(runner, command, timeout)
    go = "go 0x9000000098000040 0x900000000a000000"
    runner.emit_line(f"[ls2k-gdb] >>> {go}")
    runner.write(go.encode("ascii") + b"\r")
    state["handed_off"] = True
    state["factory_returned"] = False
    runner.wait_for_moss_shell(240)


def reset_to_factory(
    runner: GdbSerialRunner, timeout: float
) -> None:
    runner.emit_line("[ls2k-gdb] exiting site init for automatic reset")
    runner.write(b"\x03\r")
    prompt_deadline = time.monotonic() + 0.5
    while time.monotonic() < prompt_deadline:
        chunk = runner.read_and_emit(4096)
        if b"\x1b[6n" in chunk:
            runner.write(b"\x1b[1;1R")
    runner.write(b"exit\r")
    runner.wait_for_linux_shell(timeout)
    runner.emit_line("[ls2k-gdb] PASS automatic reset returned to factory Linux")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True, help="TFTP filename")
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument(
        "--device",
        default="/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0",
    )
    parser.add_argument("--root-device", default="/dev/sda3")
    parser.add_argument("--http-port", type=int, default=18089)
    parser.add_argument("--package-floor", type=int, default=15)
    parser.add_argument("--hard-deadline", type=int, default=900)
    parser.add_argument("--linux-timeout", type=float, default=600)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.root_device != "/dev/sda3":
        raise SystemExit("native GDB acceptance requires /dev/sda3")
    if not re.fullmatch(r"[A-Za-z0-9._-]+", args.image):
        raise SystemExit("--image must be a plain TFTP filename")
    if not 60 <= args.hard_deadline <= 3600:
        raise SystemExit("--hard-deadline must be in [60, 3600]")
    if not Path(args.device).exists():
        raise SystemExit(f"missing serial device: {args.device}")
    if not args.bundle.is_file() or args.bundle.stat().st_size == 0:
        raise SystemExit(f"missing bundle: {args.bundle}")
    args.log.parent.mkdir(parents=True, exist_ok=True)
    log_file: BinaryIO | None = args.log.open("ab", buffering=0)
    runner: GdbSerialRunner | None = None
    server: ThreadingHTTPServer | None = None
    server_thread: threading.Thread | None = None
    state = {
        "guarded": False,
        "handed_off": False,
        "factory_returned": True,
    }
    try:
        handler = partial(QuietHandler, directory=str(args.bundle.parent.resolve()))
        QuietHandler.allowed_name = args.bundle.name
        server = ThreadingHTTPServer((HOST_IP, args.http_port), handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        bundle_url = f"http://{HOST_IP}:{args.http_port}/{args.bundle.name}"
        digest = hashlib.sha256(args.bundle.read_bytes()).hexdigest()
        emit_line = f"[ls2k-gdb] bundle_sha256={digest} url={bundle_url}"
        runner = GdbSerialRunner(args.device, log_file)
        runner.emit_line(emit_line)
        runner.wait_for_linux_shell(args.linux_timeout)
        runner.linux_status_command(
            "grep -i -m1 '^features' /proc/cpuinfo || true", timeout=20
        )
        stage_bundle(
            runner,
            args.root_device,
            bundle_url,
            args.package_floor,
        )
        for phase in ("install", "acceptance"):
            boot_moss(
                runner,
                image_name=args.image,
                root_device=args.root_device,
                hard_deadline=args.hard_deadline,
                state=state,
            )
            if phase == "install":
                ensure_gdb_packages(runner)
            else:
                run_gdb_native_test(runner)
            rc, _ = runner.shell_command("sync", timeout=300)
            if rc != 0:
                raise RuntimeError(f"LS2K1000LA {phase} sync returned {rc}")
            reset_to_factory(runner, args.hard_deadline + 120)
            state["factory_returned"] = True
            state["handed_off"] = False
            state["guarded"] = False
            verify_filesystem(runner, args.root_device)
            runner.emit_line(
                f"[ls2k-gdb] PASS {phase} cold boot/reset/filesystem"
            )
        runner.emit_line(
            "[ls2k-gdb] ALL LS2K1000LA LOONGARCH64 NATIVE GDB HARDWARE "
            "TESTS PASSED reset=automatic filesystem=clean"
        )
        return 0
    except BaseException as error:
        if runner is not None:
            runner.emit_line(
                f"[ls2k-gdb] TEST FAILED: {type(error).__name__}: {error}"
            )
        if (
            runner is not None
            and state["guarded"]
            and not state["factory_returned"]
        ):
            try:
                if state["handed_off"]:
                    try:
                        runner.shell_command("sync", timeout=300)
                        reset_to_factory(runner, args.hard_deadline + 120)
                    except Exception as reset_error:
                        runner.emit_line(
                            f"[ls2k-gdb] command recovery unavailable: "
                            f"{reset_error}; waiting for hardware watchdog"
                        )
                        runner.wait_for_linux_shell(args.hard_deadline + 120)
                else:
                    runner.emit_line(
                        "[ls2k-gdb] waiting for pre-handoff hardware "
                        "watchdog recovery"
                    )
                    runner.wait_for_linux_shell(args.hard_deadline + 120)
                state["factory_returned"] = True
                verify_filesystem(runner, args.root_device)
                runner.emit_line(
                    "[ls2k-gdb] ABANDONED failed phase; board recovered automatically"
                )
            except Exception as recovery_error:
                runner.emit_line(
                    "[ls2k-gdb] ABANDON LS2K1000LA: automatic recovery "
                    f"unavailable; do not attempt manual reset ({recovery_error})"
                )
        return 2
    finally:
        if runner is not None:
            runner.close()
        if server is not None:
            server.shutdown()
            server.server_close()
        if server_thread is not None:
            server_thread.join(timeout=2)
        if log_file is not None:
            log_file.close()


if __name__ == "__main__":
    raise SystemExit(main())
