#!/usr/bin/env python3
"""Boot MOSS on VisionFive 2 and exercise GMAC1 through the serial shell.

The runner never executes U-Boot ``saveenv``. Run it on the host directly
connected to VF2 GMAC1. Host link settings, administrative state, and any
pre-existing TFTP target are restored before the script exits.
"""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from typing import BinaryIO

import serial


class SerialRunner:
    def __init__(self, device: str, baud: int, log_file: BinaryIO | None) -> None:
        self.serial = serial.Serial(device, baud, timeout=0.02, write_timeout=2)
        self.log_file = log_file

    def close(self) -> None:
        self.serial.close()

    def write(self, data: bytes) -> None:
        self.serial.write(data)

    def write_moss(self, data: bytes) -> None:
        # VF2 deliberately keeps the firmware-stale UART RX interrupt masked
        # and lets N_TTY poll its 8250 FIFO. Pace automated/pasted commands so
        # the 16-byte hardware FIFO cannot fill between scheduler ticks.
        for byte in data:
            self.serial.write(bytes((byte,)))
            time.sleep(0.002)

    def emit(self, data: bytes) -> None:
        if not data:
            return
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        if self.log_file is not None:
            self.log_file.write(data)
            self.log_file.flush()

    def read(self, size: int = 4096) -> bytes:
        data = self.serial.read(size)
        self.emit(data)
        return data

    def message(self, text: str) -> None:
        self.emit(f"\n[vf2-gmac] {text}\n".encode())

    def wait_for_uboot(self, timeout: float) -> None:
        self.message("reset or power-cycle the board now; interrupting autoboot")
        deadline = time.monotonic() + timeout
        tail = b""
        interrupt_sent = False
        # A single pending character is enough to stop autoboot even if the
        # adapter queues it before U-Boot enables the UART. Repeated Ctrl-C
        # bytes can instead spill into the command line and corrupt setenv.
        self.write(b"\r")
        while time.monotonic() < deadline:
            chunk = self.read()
            tail = (tail + chunk)[-8192:]
            if b"StarFive #" in tail:
                quiet_deadline = time.monotonic() + 0.2
                while time.monotonic() < quiet_deadline:
                    pending = self.read()
                    if pending:
                        quiet_deadline = time.monotonic() + 0.2
                return
            # This U-Boot clears early UART input before bootdelay starts, so
            # send the one interrupt only after the countdown prompt itself.
            if not interrupt_sent and b"Hit any key to stop autoboot" in tail:
                self.write(b"\x03")
                interrupt_sent = True
        raise TimeoutError("StarFive U-Boot prompt not observed")

    def uboot_command(self, line: str, timeout: float = 20) -> bytes:
        self.message(f">>> U-Boot: {line}")
        self.serial.reset_input_buffer()
        marker = b"__VF2_UBOOT_DONE__"
        # Ctrl-U clears any partial input left by the autoboot interrupt.
        self.write(b"\x15" + line.encode("ascii") + b"; echo " + marker + b"\r")
        deadline = time.monotonic() + timeout
        tail = b""
        while time.monotonic() < deadline:
            tail = (tail + self.read())[-65536:]
            if tail.count(marker) >= 2 and b"StarFive #" in tail:
                errors = (
                    b"Unknown command",
                    b"*** ERROR:",
                    b"TFTP error",
                    b"Retry count exceeded",
                )
                if any(error in tail for error in errors):
                    raise RuntimeError(f"U-Boot command failed: {line}")
                return tail
        raise TimeoutError(f"U-Boot command timed out: {line}")

    def moss_command(self, line: str, timeout: float = 30) -> tuple[int, bytes]:
        self.message(f">>> MOSS: {line}")
        self.write(b"\x03\r")
        prompt_deadline = time.monotonic() + 0.5
        while time.monotonic() < prompt_deadline:
            chunk = self.read()
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
        command = (
            line
            + "; __vf2_rc=$?; printf '\n__VF2_RC_%s__\n' \"$__vf2_rc\"\r"
        )
        self.write_moss(command.encode("ascii"))
        deadline = time.monotonic() + timeout
        tail = b""
        marker = re.compile(rb"__VF2_RC_(\d+)__")
        while time.monotonic() < deadline:
            chunk = self.read()
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
            tail = (tail + chunk)[-131072:]
            match = marker.search(tail)
            if match is not None:
                return int(match.group(1)), tail
        raise TimeoutError(f"MOSS shell command timed out: {line}")

    def wait_for_moss_shell(self, timeout: float) -> None:
        deadline = time.monotonic() + timeout
        tail = b""
        while time.monotonic() < deadline:
            chunk = self.read()
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
            tail = (tail + chunk)[-8192:]
            if b"starry:~#" in tail:
                rc, _ = self.moss_command("echo VF2_SHELL_READY", timeout=5)
                if rc == 0:
                    return
        raise TimeoutError("MOSS shell did not become ready")


class TcpReceiver:
    """Receive an exact payload without depending on host netcat EOF semantics."""

    def __init__(self, host: str, port: int, expected_size: int, timeout: float) -> None:
        self.expected_size = expected_size
        self.timeout = timeout
        self.data = bytearray()
        self.error: BaseException | None = None
        self.done = threading.Event()
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.settimeout(timeout)
        self.listener.bind((host, port))
        self.listener.listen(1)
        self.thread = threading.Thread(target=self._receive, daemon=True)
        self.thread.start()

    def _receive(self) -> None:
        try:
            connection, _ = self.listener.accept()
            with connection:
                connection.settimeout(self.timeout)
                while len(self.data) < self.expected_size:
                    chunk = connection.recv(self.expected_size - len(self.data))
                    if not chunk:
                        break
                    self.data.extend(chunk)
        except BaseException as error:
            self.error = error
        finally:
            self.listener.close()
            self.done.set()

    def collect(self, timeout: float) -> bytes:
        if not self.done.wait(timeout):
            raise TimeoutError("host TCP receiver timed out")
        if self.error is not None:
            raise self.error
        return bytes(self.data)

    def close(self) -> None:
        self.listener.close()
        self.thread.join(timeout=1)


def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(command, check=True, **kwargs)


def interface_has_address(interface: str, address: str) -> bool:
    result = subprocess.run(
        ["ip", "-o", "-4", "address", "show", "dev", interface],
        check=False,
        stdout=subprocess.PIPE,
        text=True,
    )
    return any(field.split("/")[0] == address for field in result.stdout.split())


def interface_is_up(interface: str) -> bool:
    result = run(
        ["ip", "-o", "link", "show", "dev", interface],
        stdout=subprocess.PIPE,
    ).stdout.decode()
    flags = re.search(r"<([^>]*)>", result)
    return flags is not None and "UP" in flags.group(1).split(",")


def wait_host_link(interface: str, timeout: float = 20) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        output = subprocess.run(
            ["ethtool", interface], check=False, stdout=subprocess.PIPE, text=True
        ).stdout
        if "Link detected: yes" in output:
            return
        time.sleep(0.5)
    raise TimeoutError(f"host interface {interface} did not regain carrier")


def advertised_mask(output: str) -> int:
    modes = {
        "10baseT/Half": 0x001,
        "10baseT/Full": 0x002,
        "100baseT/Half": 0x004,
        "100baseT/Full": 0x008,
        "1000baseT/Half": 0x010,
        "1000baseT/Full": 0x020,
    }
    mask = 0
    advertised = re.search(
        r"Advertised link modes:\s*(.*?)(?:\n\s*Advertised pause frame use:)",
        output,
        re.DOTALL,
    )
    text = advertised.group(1) if advertised else ""
    for name, bit in modes.items():
        if name in text:
            mask |= bit
    pause = re.search(r"Advertised pause frame use:\s*(.*)", output)
    if pause:
        value = pause.group(1)
        if "Symmetric" in value:
            mask |= 0x040
        if "Receive-only" in value or "Transmit-only" in value:
            mask |= 0x080
    return mask


class LinkSettings:
    def __init__(self, interface: str) -> None:
        self.interface = interface
        self.output = run(["ethtool", interface], stdout=subprocess.PIPE).stdout.decode()
        self.autoneg = "Auto-negotiation: on" in self.output
        speed = re.search(r"Speed:\s*(\d+)Mb/s", self.output)
        duplex = re.search(r"Duplex:\s*(Full|Half)", self.output)
        self.speed = speed.group(1) if speed else None
        self.duplex = duplex.group(1).lower() if duplex else None
        self.advertise = advertised_mask(self.output)

    def restore(self) -> None:
        if self.autoneg:
            command = ["ethtool", "-s", self.interface, "autoneg", "on"]
            if self.advertise:
                command += ["advertise", hex(self.advertise)]
        else:
            if self.speed is None or self.duplex is None:
                raise RuntimeError("cannot restore original forced link settings")
            command = [
                "ethtool",
                "-s",
                self.interface,
                "speed",
                self.speed,
                "duplex",
                self.duplex,
                "autoneg",
                "off",
            ]
        run(command)


class TftpDeployment:
    """Temporarily deploy an image without destroying an existing TFTP file."""

    def __init__(self, source: Path, target: Path) -> None:
        self.source = source
        self.target = target
        self.changed = source.resolve() != target.resolve()
        self.backup: Path | None = None
        self.created = False

    def deploy(self) -> None:
        if not self.changed:
            return
        if self.target.is_symlink():
            raise RuntimeError(f"refusing to overwrite symlinked TFTP target: {self.target}")
        if self.target.exists():
            descriptor, name = tempfile.mkstemp(
                prefix=f".{self.target.name}.vf2-backup-", dir=self.target.parent
            )
            os.close(descriptor)
            self.backup = Path(name)
            shutil.copy2(self.target, self.backup)
        else:
            self.created = True
        shutil.copy2(self.source, self.target)

    def restore(self) -> None:
        if not self.changed:
            return
        if self.backup is not None:
            os.replace(self.backup, self.target)
            self.backup = None
        elif self.created:
            self.target.unlink(missing_ok=True)
            self.created = False


def require_moss_command(
    runner: SerialRunner, label: str, command: str, timeout: float = 30
) -> bytes:
    rc, output = runner.moss_command(command, timeout)
    if rc != 0:
        raise RuntimeError(f"{label} failed with exit status {rc}")
    runner.message(f"PASS {label}")
    return output


def wait_board_network(runner: SerialRunner, host_ip: str, timeout: float = 30) -> None:
    payload = b"VF2_LINK_READY"
    port = 18999
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        receiver = TcpReceiver(host_ip, port, len(payload), timeout=3)
        rc = -1
        try:
            rc, _ = runner.moss_command(
                f"printf VF2_LINK_READY | busybox nc -w 2 {host_ip} {port}",
                timeout=5,
            )
            received = receiver.collect(timeout=3)
        except (OSError, TimeoutError):
            received = b""
        finally:
            receiver.close()
        if rc == 0 and received == payload:
            runner.message("PASS link/ARP/TCP")
            return
        time.sleep(1)
    raise TimeoutError("MOSS could not open TCP to the directly connected host")


def network_roundtrip(
    runner: SerialRunner,
    host_ip: str,
    board_ip: str,
    suffix: int,
) -> None:
    size = 65536
    tx_port = 19000 + suffix * 2
    rx_port = tx_port + 1

    host_receiver = TcpReceiver(host_ip, tx_port, size, timeout=15)
    try:
        require_moss_command(
            runner,
            "MOSS-to-host TCP",
            f"head -c {size} /dev/zero | tr '\\000' V | busybox nc -w 5 {host_ip} {tx_port}",
            timeout=15,
        )
        received = host_receiver.collect(timeout=10)
    finally:
        host_receiver.close()
    expected = b"V" * size
    if received != expected:
        raise RuntimeError(
            f"MOSS-to-host payload mismatch: got {len(received)} bytes, "
            f"sha256={hashlib.sha256(received).hexdigest()}"
        )
    runner.message(f"PASS MOSS-to-host payload sha256={hashlib.sha256(expected).hexdigest()}")

    remote_file = f"/tmp/vf2-gmac-rx-{suffix}"
    remote_pid = f"{remote_file}.pid"
    require_moss_command(
        runner,
        "start MOSS TCP listener",
        f"rm -f {remote_file} {remote_pid}; "
        f"busybox nc -l -p {rx_port} > {remote_file} & echo $! > {remote_pid}",
    )
    payload = b"R" * size
    run(["nc", "-w", "5", board_ip, str(rx_port)], input=payload)
    output = require_moss_command(
        runner,
        "host-to-MOSS TCP",
        f"pid=$(cat {remote_pid}); wait \"$pid\"; "
        f"busybox sha256sum {remote_file}; busybox wc -c < {remote_file}",
    ).decode("ascii", errors="replace")
    digest = hashlib.sha256(payload).hexdigest()
    if digest not in output or str(size) not in output:
        raise RuntimeError("host-to-MOSS payload length or sha256 mismatch")
    runner.message(f"PASS host-to-MOSS payload sha256={digest}")


def run_apk_test(runner: SerialRunner, package: str) -> None:
    require_moss_command(
        runner,
        "DNS",
        "busybox nslookup dl-cdn.alpinelinux.org",
        timeout=20,
    )
    require_moss_command(
        runner,
        "TLS",
        "busybox wget -q -O /dev/null https://dl-cdn.alpinelinux.org/alpine/",
        timeout=45,
    )
    require_moss_command(runner, "apk index update", "apk update", timeout=120)
    quoted = shlex.quote(package)
    require_moss_command(
        runner,
        "apk package fetch",
        f"rm -rf /tmp/vf2-apk; mkdir -p /tmp/vf2-apk; "
        f"apk fetch --output /tmp/vf2-apk {quoted}; "
        "busybox find /tmp/vf2-apk -type f -name '*.apk' | busybox grep -q .",
        timeout=120,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", type=Path, default=Path("moss-vf2.bin"))
    parser.add_argument("--tftp-dir", type=Path, default=Path("/srv/tftp"))
    parser.add_argument("--tftp-name", default="kernel")
    parser.add_argument("--serial", default="/dev/ttyACM0")
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument("--interface", default="enp3s0")
    parser.add_argument("--host-ip", type=ipaddress.IPv4Address, default="192.168.88.1")
    parser.add_argument("--board-ip", type=ipaddress.IPv4Address, default="192.168.88.2")
    parser.add_argument("--root-device", default="/dev/mmcblk1p2")
    parser.add_argument("--uboot-timeout", type=float, default=120)
    parser.add_argument("--boot-timeout", type=float, default=120)
    parser.add_argument("--skip-link-flap", action="store_true")
    parser.add_argument("--speed-suite", action="store_true")
    parser.add_argument("--apk-package")
    parser.add_argument("--log", type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    host_ip = str(args.host_ip)
    board_ip = str(args.board_ip)
    image = args.image.resolve()
    if not image.is_file() or image.stat().st_size == 0:
        raise SystemExit(f"missing VF2 image: {image}")
    if not Path(args.serial).exists():
        raise SystemExit(f"missing serial device: {args.serial}")
    for tool in ("ip", "ethtool", "nc"):
        if shutil.which(tool) is None:
            raise SystemExit(f"missing host tool: {tool}")
    if (
        not re.fullmatch(r"[A-Za-z0-9._-]+", args.tftp_name)
        or args.tftp_name in (".", "..")
    ):
        raise SystemExit("--tftp-name must be a plain U-Boot/TFTP file name")
    if args.apk_package and not re.fullmatch(
        r"[a-z0-9][a-z0-9+_.-]*", args.apk_package
    ):
        raise SystemExit("--apk-package is not a valid Alpine package name")
    if not re.fullmatch(r"/dev/mmcblk[0-9]+p[0-9]+", args.root_device):
        raise SystemExit("--root-device must name one SD/MMC partition")
    if not interface_has_address(args.interface, host_ip):
        raise SystemExit(
            f"{args.interface} does not own {host_ip}; run scripts/setup_vf2_network.sh first"
        )
    if (not args.skip_link_flap or args.speed_suite) and os.geteuid() != 0:
        raise SystemExit("link flap/speed tests require root; use sudo or --skip-link-flap")

    tftp_target = args.tftp_dir / args.tftp_name
    if not args.tftp_dir.is_dir() or not os.access(args.tftp_dir, os.W_OK):
        raise SystemExit(f"TFTP directory is missing or not writable: {args.tftp_dir}")
    deployment = TftpDeployment(image, tftp_target)
    original_admin_up = interface_is_up(args.interface)
    original_link = LinkSettings(args.interface) if args.speed_suite else None
    link_settings_dirty = False
    log_file: BinaryIO | None = None
    runner: SerialRunner | None = None
    try:
        deployment.deploy()
        log_file = open(args.log, "ab", buffering=0) if args.log else None
        runner = SerialRunner(args.serial, args.baud, log_file)
        runner.wait_for_uboot(args.uboot_timeout)
        for command, timeout in (
            (f"setenv ipaddr {board_ip}", 5),
            (f"setenv serverip {host_ip}", 5),
            ("setenv netmask 255.255.255.0", 5),
            (f"tftpboot 0x40200000 {args.tftp_name}", 45),
            ("fdt addr 0x42200000", 5),
            (f'fdt set /chosen bootargs "root={args.root_device} rw"', 5),
        ):
            runner.uboot_command(command, timeout)
        runner.message(">>> U-Boot: go 0x40200000")
        runner.write(b"go 0x40200000\r")
        runner.wait_for_moss_shell(args.boot_timeout)
        wait_board_network(runner, host_ip)
        network_roundtrip(runner, host_ip, board_ip, 0)

        if not args.skip_link_flap:
            runner.message("flapping the host link")
            run(["ip", "link", "set", args.interface, "down"])
            time.sleep(2)
            run(["ip", "link", "set", args.interface, "up"])
            wait_host_link(args.interface)
            wait_board_network(runner, host_ip)
            network_roundtrip(runner, host_ip, board_ip, 1)

        if args.speed_suite:
            assert original_link is not None
            speed_advertise = ((1_000, 0x0E0), (100, 0x0C8), (10, 0x0C2))
            try:
                link_settings_dirty = True
                for index, (speed, mask) in enumerate(speed_advertise, start=2):
                    runner.message(f"advertising only {speed}baseT/Full on host")
                    run(
                        [
                            "ethtool",
                            "-s",
                            args.interface,
                            "advertise",
                            hex(mask),
                            "autoneg",
                            "on",
                        ]
                    )
                    wait_host_link(args.interface, 30)
                    wait_board_network(runner, host_ip, 30)
                    network_roundtrip(runner, host_ip, board_ip, index)
            finally:
                original_link.restore()
                link_settings_dirty = False
                wait_host_link(args.interface)

        if args.apk_package:
            run_apk_test(runner, args.apk_package)
        runner.message("ALL REQUESTED VF2 GMAC TESTS PASSED")
    finally:
        if link_settings_dirty and original_link is not None:
            try:
                original_link.restore()
            except Exception as error:  # preserve the primary failure
                print(f"warning: failed to restore link settings: {error}", file=sys.stderr)
        if not args.skip_link_flap:
            try:
                subprocess.run(
                    [
                        "ip",
                        "link",
                        "set",
                        args.interface,
                        "up" if original_admin_up else "down",
                    ],
                    check=False,
                )
            except Exception as error:
                print(f"warning: failed to bring {args.interface} up: {error}", file=sys.stderr)
        try:
            if runner is not None:
                runner.close()
        finally:
            if log_file is not None:
                log_file.close()
            try:
                deployment.restore()
            except Exception as error:
                print(f"warning: failed to restore TFTP target: {error}", file=sys.stderr)


if __name__ == "__main__":
    main()
