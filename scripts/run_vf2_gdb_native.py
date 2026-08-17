#!/usr/bin/env python3
"""Run the native RISC-V GDB acceptance on a VisionFive 2 board.

The board is always handed off through U-Boot with a hardware watchdog.  The
first MOSS boot installs the offline packages and the second cold boot runs the
demo.  Debian p1 verifies the Alpine target partition after both boots.
"""

from __future__ import annotations

import argparse
from functools import partial
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import re
import shutil
import subprocess
import sys
import threading
import time
from typing import BinaryIO

from gdb_native_workload import ensure_gdb_packages, run_gdb_native_test
from run_vf2_gmac_test import SerialRunner


HOST_IP = "192.168.1.2"
BOARD_IP = "192.168.1.10"
RELAY = "/usr/local/sbin/moss-vf2-relay"


class QuietHandler(SimpleHTTPRequestHandler):
    allowed_name = ""

    def log_message(self, format: str, *args: object) -> None:
        return

    def do_GET(self) -> None:
        if self.path != f"/{self.allowed_name}":
            self.send_error(404)
            return
        super().do_GET()


class WorkloadAdapter:
    def __init__(self, runner: SerialRunner) -> None:
        self.runner = runner

    def shell_command(
        self, line: str, timeout: float = 20
    ) -> tuple[int, bytes]:
        return self.runner.moss_command(line, timeout)

    def emit_line(self, message: str) -> None:
        self.runner.message(message)


def emit(log_file: BinaryIO | None, message: str) -> None:
    data = f"[vf2-gdb] {message}\n".encode()
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
    if log_file is not None:
        log_file.write(data)
        log_file.flush()


def run(
    command: list[str], *, timeout: float | None = None
) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
    )


def discover_serial() -> str:
    candidates = sorted(
        Path("/dev/serial/by-id").glob("usb-1a86_USB_Single_Serial_*-if00")
    )
    if len(candidates) == 1:
        return str(candidates[0])
    if Path("/dev/ttyACM0").exists():
        return "/dev/ttyACM0"
    raise RuntimeError("cannot uniquely discover the VF2 USB serial device")


def discover_interface() -> str:
    candidates: list[str] = []
    for interface in Path("/sys/class/net").iterdir():
        try:
            if (interface / "device/driver").resolve().name == "r8152":
                candidates.append(interface.name)
        except FileNotFoundError:
            continue
    if len(candidates) != 1:
        raise RuntimeError(
            "cannot uniquely discover the VF2 r8152 host interface; use --interface"
        )
    return candidates[0]


def ensure_host_network(interface: str) -> None:
    run(["sudo", "-n", "ip", "link", "set", "dev", interface, "up"])
    run(
        [
            "sudo",
            "-n",
            "ip",
            "address",
            "replace",
            f"{HOST_IP}/24",
            "dev",
            interface,
        ]
    )


def ssh_command(
    username: str,
    line: str,
    log_file: BinaryIO | None,
    *,
    timeout: float,
    allow_disconnect: bool = False,
) -> bytes:
    result = subprocess.run(
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            f"{username}@{BOARD_IP}",
            line,
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
    )
    if result.stdout:
        sys.stdout.buffer.write(result.stdout)
        sys.stdout.buffer.flush()
        if log_file is not None:
            log_file.write(result.stdout)
            log_file.flush()
    if result.returncode != 0 and not (
        allow_disconnect and result.returncode == 255
    ):
        raise RuntimeError(
            f"VF2 SSH command returned {result.returncode}: {line}"
        )
    return result.stdout


def wait_for_ssh(
    username: str,
    interface: str,
    log_file: BinaryIO | None,
    timeout: float,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            ensure_host_network(interface)
            ssh_command(username, "true", None, timeout=8)
            output = ssh_command(
                username,
                f"sudo -n {RELAY} status",
                log_file,
                timeout=20,
            )
            if b"[vf2-relay] READY" in output:
                emit(log_file, "PASS Debian p1 SSH relay ready")
                return
        except (OSError, RuntimeError, subprocess.TimeoutExpired):
            time.sleep(2)
    raise TimeoutError("VF2 Debian p1 SSH relay did not become ready")


def deploy_tftp(image: Path, tftp_dir: Path) -> tuple[str, str]:
    sockets = run(["ss", "-H", "-lun"]).stdout
    if re.search(rb"(?:^|\s)(?:\[[^]]+\]|[^\s]+):69(?:\s|$)", sockets) is None:
        raise RuntimeError("no host TFTP server is listening on UDP port 69")
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    name = f"moss-vf2-gdb-{digest[:16]}.bin"
    target = tftp_dir / name
    run(
        [
            "sudo",
            "-n",
            "install",
            "-o",
            "tftp",
            "-g",
            "tftp",
            "-m",
            "0644",
            str(image),
            str(target),
        ]
    )
    if hashlib.sha256(target.read_bytes()).hexdigest() != digest:
        raise RuntimeError("deployed TFTP image hash mismatch")
    return name, digest


def uboot_read_u32(runner: SerialRunner, address: int) -> int:
    output = runner.uboot_command(f"md.l 0x{address:x} 1", 5)
    match = re.search(
        rb"\b" + f"{address:08x}".encode() + rb":\s+([0-9a-fA-F]{8})\b",
        output,
        re.IGNORECASE,
    )
    if match is None:
        raise RuntimeError(f"cannot parse U-Boot value at 0x{address:x}")
    return int(match.group(1), 16)


def arm_uboot_watchdog(runner: SerialRunner) -> None:
    for address in (0x1302_01E8, 0x1302_01EC):
        value = uboot_read_u32(runner, address) | 0x8000_0000
        runner.uboot_command(f"mw.l 0x{address:x} 0x{value:08x}", 5)
    reset_address = 0x1302_0304
    reset_value = uboot_read_u32(runner, reset_address) & ~0x6000
    runner.uboot_command(f"mw.l 0x{reset_address:x} 0x{reset_value:08x}", 5)
    for command in (
        "mw.l 0x13070c00 0x1acce551",
        "mw.l 0x13070008 0",
        "mw.l 0x13070008 2",
        "mw.l 0x1307000c 1",
        "mw.l 0x13070000 0x15752a00",
        "mw.l 0x1307000c 1",
        "mw.l 0x13070008 3",
        "mw.l 0x13070c00 0xe5331aae",
    ):
        runner.uboot_command(command, 5)
    runner.message("JH7110 handoff watchdog armed for 30 seconds")


def tftp_boot(runner: SerialRunner, interface: str, name: str) -> None:
    for attempt in range(1, 4):
        ensure_host_network(interface)
        try:
            runner.uboot_command(f"tftpboot 0x40200000 {name}", 60)
            return
        except (RuntimeError, TimeoutError):
            if attempt == 3:
                raise
            runner.message(f"TFTP attempt {attempt} failed; rearming watchdog")
            arm_uboot_watchdog(runner)


def wait_for_linux_return(
    runner: SerialRunner, interface: str, timeout: float
) -> None:
    deadline = time.monotonic() + timeout
    tail = b""
    while time.monotonic() < deadline:
        tail = (tail + runner.read())[-262144:]
        if b"visionfive2 login:" in tail:
            ensure_host_network(interface)
            runner.message("PASS hardware reset returned to Debian login")
            return
    raise TimeoutError("hardware reset did not return to Debian login")


def request_graceful_reboot(runner: SerialRunner) -> None:
    runner.write(b"\x03\r")
    deadline = time.monotonic() + 0.75
    while time.monotonic() < deadline:
        chunk = runner.read()
        if b"\x1b[6n" in chunk:
            runner.write(b"\x1b[1;1R")
    runner.write_moss(
        b"if command -v reboot >/dev/null 2>&1; then exec reboot; "
        b"else exec /musl/busybox reboot; fi\r"
    )


def stage_bundle(
    adapter: WorkloadAdapter, bundle_url: str, package_floor: int
) -> None:
    rc, output = adapter.shell_command(
        "rm -rf /var/cache/moss-gdb-native "
        "/var/cache/moss-gdb-native.tar.new && "
        "mkdir -p /var/cache/moss-gdb-native && "
        f"busybox wget -O /var/cache/moss-gdb-native.tar.new {bundle_url} && "
        "tar -tf /var/cache/moss-gdb-native.tar.new >/dev/null && "
        "tar -xf /var/cache/moss-gdb-native.tar.new "
        "-C /var/cache/moss-gdb-native && "
        "rm /var/cache/moss-gdb-native.tar.new && "
        "test -f /var/cache/moss-gdb-native/gdb-native-demo.c && "
        "test -f /var/cache/moss-gdb-native/gdb-native-smoke.gdb && "
        "test \"$(find /var/cache/moss-gdb-native -maxdepth 1 "
        f"-name '*.apk' | wc -l)\" -ge {package_floor} && "
        "apk verify /var/cache/moss-gdb-native/*.apk",
        timeout=600,
    )
    if rc != 0:
        raise RuntimeError(f"VF2 GDB bundle staging failed with status {rc}")
    if b"gdb-native-demo.c" not in output:
        adapter.emit_line("[gdb-native] PASS offline GDB bundle staged and verified")


def run_phase(
    *,
    runner: SerialRunner,
    adapter: WorkloadAdapter,
    phase: str,
    image_name: str,
    bundle_url: str,
    package_floor: int,
    interface: str,
    username: str,
    root_device: str,
    hard_deadline: int,
    log_file: BinaryIO | None,
    state: dict[str, bool],
) -> None:
    ssh_command(
        username,
        f"sudo -n {RELAY} prepare {root_device} {int(time.time())}",
        log_file,
        timeout=600,
    )
    ssh_command(
        username,
        f"sudo -n {RELAY} reboot",
        log_file,
        timeout=30,
        allow_disconnect=True,
    )
    runner.wait_for_uboot(120)
    ensure_host_network(interface)
    arm_uboot_watchdog(runner)
    state["guarded"] = True
    state["linux_returned"] = False
    for command, timeout in (
        (f"setenv ipaddr {BOARD_IP}", 5),
        (f"setenv serverip {HOST_IP}", 5),
        ("setenv netmask 255.255.255.0", 5),
    ):
        runner.uboot_command(command, timeout)
    tftp_boot(runner, interface, image_name)
    for command, timeout in (
        ("fdt addr 0x42200000", 5),
        ("fdt resize 4096", 5),
        (f'fdt set /chosen bootargs "root={root_device} rw"', 5),
        (
            "fdt set /chosen moss,watchdog-hard-deadline-secs "
            f"<{hard_deadline}>",
            5,
        ),
    ):
        runner.uboot_command(command, timeout)
    runner.message(">>> U-Boot: go 0x40200000")
    runner.write(b"go 0x40200000\r")
    state["handed_off"] = True
    state["linux_returned"] = False
    runner.wait_for_moss_shell(240)
    if phase == "install":
        stage_bundle(adapter, bundle_url, package_floor)
        ensure_gdb_packages(adapter)
    else:
        run_gdb_native_test(adapter)
    rc, _ = adapter.shell_command("sync", timeout=300)
    if rc != 0:
        raise RuntimeError(f"VF2 {phase} sync failed with status {rc}")
    request_graceful_reboot(runner)
    wait_for_linux_return(runner, interface, hard_deadline + 120)
    state["linux_returned"] = True
    wait_for_ssh(username, interface, log_file, 240)
    ssh_command(
        username,
        f"sudo -n {RELAY} verify {root_device} {int(time.time())}",
        log_file,
        timeout=600,
    )
    emit(log_file, f"PASS {phase} cold boot/reset/filesystem")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--tftp-dir", type=Path, default=Path("/srv/tftp"))
    parser.add_argument("--serial")
    parser.add_argument("--interface")
    parser.add_argument("--username", default="vf2")
    parser.add_argument("--root-device", default="/dev/mmcblk1p3")
    parser.add_argument("--http-port", type=int, default=18090)
    parser.add_argument("--package-floor", type=int, default=15)
    parser.add_argument("--hard-deadline", type=int, default=900)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.root_device not in (
        "/dev/mmcblk1p2",
        "/dev/mmcblk1p3",
        "/dev/mmcblk1p4",
    ):
        raise SystemExit("unsupported VF2 root device")
    if not 60 <= args.hard_deadline <= 3600:
        raise SystemExit("--hard-deadline must be in [60, 3600]")
    for path in (args.image, args.bundle):
        if not path.is_file() or path.stat().st_size == 0:
            raise SystemExit(f"missing input: {path}")
    for tool in ("ip", "ssh", "ss", "sudo"):
        if shutil.which(tool) is None:
            raise SystemExit(f"missing host tool: {tool}")
    run(["sudo", "-n", "true"])
    serial_device = args.serial or discover_serial()
    interface = args.interface or discover_interface()
    if not Path(serial_device).exists():
        raise SystemExit(f"missing serial device: {serial_device}")
    ensure_host_network(interface)
    args.log.parent.mkdir(parents=True, exist_ok=True)
    log_file: BinaryIO | None = args.log.open("ab", buffering=0)
    runner: SerialRunner | None = None
    server: ThreadingHTTPServer | None = None
    server_thread: threading.Thread | None = None
    state = {"guarded": False, "handed_off": False, "linux_returned": True}
    try:
        wait_for_ssh(args.username, interface, log_file, 240)
        image_name, digest = deploy_tftp(args.image.resolve(), args.tftp_dir)
        emit(log_file, f"image_sha256={digest} tftp={image_name}")
        bundle_digest = hashlib.sha256(args.bundle.read_bytes()).hexdigest()
        emit(log_file, f"bundle_sha256={bundle_digest}")
        QuietHandler.allowed_name = args.bundle.name
        handler = partial(QuietHandler, directory=str(args.bundle.parent.resolve()))
        server = ThreadingHTTPServer((HOST_IP, args.http_port), handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        bundle_url = f"http://{HOST_IP}:{args.http_port}/{args.bundle.name}"
        runner = SerialRunner(serial_device, 115200, log_file)
        adapter = WorkloadAdapter(runner)
        for phase in ("install", "acceptance"):
            run_phase(
                runner=runner,
                adapter=adapter,
                phase=phase,
                image_name=image_name,
                bundle_url=bundle_url,
                package_floor=args.package_floor,
                interface=interface,
                username=args.username,
                root_device=args.root_device,
                hard_deadline=args.hard_deadline,
                log_file=log_file,
                state=state,
            )
            state["handed_off"] = False
            state["guarded"] = False
        emit(
            log_file,
            "ALL VF2 RISCV64 NATIVE GDB HARDWARE TESTS PASSED "
            "reset=automatic filesystem=clean",
        )
        return 0
    except BaseException as error:
        emit(log_file, f"TEST FAILED: {type(error).__name__}: {error}")
        if (
            runner is not None
            and state["guarded"]
            and not state["linux_returned"]
        ):
            try:
                if state["handed_off"]:
                    runner.message("attempting bounded sync and automatic recovery")
                    try:
                        adapter = WorkloadAdapter(runner)
                        adapter.shell_command("sync", timeout=300)
                        request_graceful_reboot(runner)
                    except Exception as reset_error:
                        runner.message(
                            f"command recovery unavailable: {reset_error}; "
                            "waiting for hardware watchdog"
                        )
                else:
                    runner.message("waiting for pre-handoff hardware watchdog recovery")
                wait_for_linux_return(runner, interface, args.hard_deadline + 120)
                wait_for_ssh(args.username, interface, log_file, 240)
                ssh_command(
                    args.username,
                    f"sudo -n {RELAY} verify {args.root_device} {int(time.time())}",
                    log_file,
                    timeout=600,
                )
                emit(log_file, "ABANDONED failed phase; board recovered automatically")
            except Exception as recovery_error:
                emit(
                    log_file,
                    "ABANDON VF2: automatic recovery unavailable; "
                    f"do not attempt manual reset ({recovery_error})",
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
