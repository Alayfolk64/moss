#!/usr/bin/env python3
"""RAM-chain BSP2 U-Boot and Linux 6.12 on LS2K1000-DP.

Nothing is written to SPI flash.  The factory U-Boot first loads the BSP2
U-Boot proper at its linked RAM address.  BSP2 U-Boot then loads the candidate
Linux uImage.  The default smoke test mounts the competition root read-only.
Explicit cagent/buildstorm tests remount it read-write because the unmodified
contest scripts create files and build artifacts.  TFTP and serial YMODEM
transports are supported.  A hardware watchdog bounds both handoffs and
restores the factory boot path on failure.
"""

from __future__ import annotations

import argparse
import fcntl
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import threading
import time

from run_ls2k1000_serial_tftp import SerialRunner


UBOOT_STAGING_ADDRESS = "0x9000000098000000"
UBOOT_ENTRY_ADDRESS = "0x900000000c000000"
KERNEL_ADDRESS = "0x9000000098000000"
WATCHDOG_COUNT_30S = "0xdf847580"
EXPECTED_BSP2_BANNER = b"U-Boot 2022.04-g76bc6701"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "uboot",
        help="artifact filename of the BSP2 U-Boot standalone legacy image",
    )
    parser.add_argument("kernel", help="artifact filename of Linux 6.12 uImage")
    parser.add_argument("--device", default="/dev/ttyUSB0")
    parser.add_argument("--linux-timeout", type=float, default=600)
    parser.add_argument("--handoff-timeout", type=float, default=40)
    parser.add_argument("--recovery-monitor-seconds", type=float, default=45)
    parser.add_argument(
        "--linux-test",
        choices=("smoke", "cagent", "buildstorm", "all"),
        default="smoke",
        help=(
            "test to run after Linux reaches init=/bin/sh; workload modes "
            "write their normal temporary/build outputs to sda2"
        ),
    )
    parser.add_argument(
        "--workload-timeout",
        type=float,
        default=0,
        help=(
            "optional host-side workload deadline in seconds; 0 leaves "
            "cagent/buildstorm unlimited while the hardware watchdog is fed"
        ),
    )
    parser.add_argument(
        "--swap-device",
        default="/dev/sda1",
        help=(
            "filesystem containing the persistent swap file used by "
            "cagent/buildstorm modes"
        ),
    )
    parser.add_argument(
        "--swap-file",
        default="/swapfile-moss",
        help="absolute path to the swap file within --swap-device",
    )
    parser.add_argument(
        "--transport",
        choices=("tftp", "ymodem"),
        default="tftp",
        help="move RAM artifacts over Ethernet or the existing serial link",
    )
    # The board reports a 7,812,500 base baud.  460800 has a near-integral
    # divisor; 921600 is far enough off that this CH340 link becomes garbled.
    parser.add_argument("--ymodem-baud", type=int, default=460800)
    parser.add_argument(
        "--kernel-ymodem-chunk-size",
        type=int,
        default=120 * 1024,
        help=(
            "split only the kernel transfer into chunks of this many bytes; "
            "120 KiB stays below the BSP2 xyzModem high-bit block boundary"
        ),
    )
    parser.add_argument("--tftp-root", type=Path, default=Path("/srv/tftp"))
    parser.add_argument(
        "--resume-bsp2",
        action="store_true",
        help=(
            "resume at an already-stopped RAM-chain BSP2 U-Boot prompt; "
            "skip the factory reboot and U-Boot transfer"
        ),
    )
    return parser.parse_args()


def watchdog_commands() -> list[tuple[str, int]]:
    return [
        ("mw.l 0x1fe27030 0", 5),
        (f"mw.l 0x1fe27038 {WATCHDOG_COUNT_30S}", 5),
        ("mw.l 0x1fe27030 2", 5),
        ("mw.l 0x1fe27034 1", 5),
    ]


def arm_watchdog(runner: SerialRunner) -> None:
    for command, command_timeout in watchdog_commands():
        runner.command(command, command_timeout)


def tftp_required(
    runner: SerialRunner,
    address: str,
    filename: str,
    *,
    attempts: int = 3,
) -> None:
    """TFTP a fresh artifact or fail before any stale RAM can be executed."""
    for attempt in range(1, attempts + 1):
        arm_watchdog(runner)
        output = runner.command(
            f"tftpboot {address} {filename}",
            30,
        )
        if b"Bytes transferred =" in output:
            return
        print(
            f"\n[bsp2-chain] TFTP failed ({attempt}/{attempts}); "
            "reinitializing the selected PHY",
            flush=True,
        )
        # Keep a failed primary PHY from consuming another full timeout on a
        # different board port.  These environment changes remain volatile.
        runner.command("setenv ethrotate no", 5)
        runner.command("setenv ethact ethernet@40040000", 5)
    raise RuntimeError(
        f"TFTP did not transfer {filename!r} after {attempts} attempts"
    )


def ymodem_required(
    runner: SerialRunner,
    address: str,
    path: Path,
    baud: int,
) -> None:
    """Load one local artifact into U-Boot RAM without Ethernet or storage."""
    path = path.resolve(strict=True)
    expected_size = path.stat().st_size
    print(
        f"\n[bsp2-chain] >>> loady {address} {baud}; "
        f"send {path} ({expected_size} bytes)",
        flush=True,
    )
    original_baud = runner.serial.baudrate
    runner.write(f"loady {address} {baud}\r".encode("ascii"))
    deadline = time.monotonic() + 10
    tail = b""
    switched_baud = False
    while time.monotonic() < deadline:
        tail = (tail + runner.read_and_emit())[-8192:]
        if b"Ready for binary (ymodem)" in tail:
            break
        if b"Switch baudrate to" in tail and b"press ENTER" in tail:
            switched_baud = True
            break
    else:
        raise TimeoutError("U-Boot did not enter YMODEM setup")

    if switched_baud:
        # U-Boot changes its UART first and then waits for an ENTER at the new
        # speed.  Reconfigure the already-open pyserial descriptor for that
        # handshake.  Its implementation delays for 50 ms on each side of
        # serial_setbrg(), so do not let ENTER land at the old speed.
        time.sleep(0.15)
        runner.serial.baudrate = baud
        runner.write(b"\r")
        deadline = time.monotonic() + 10
        tail = b""
        while time.monotonic() < deadline:
            tail = (tail + runner.read_and_emit())[-8192:]
            if b"Ready for binary (ymodem)" in tail:
                break
        else:
            raise TimeoutError("U-Boot did not enter YMODEM receive mode")

    # The board-side xyzModem loop ACKs each 1 KiB packet in software.  A
    # measured 460800-baud run sustains about 10.6 KiB/s, not the UART's raw
    # 46 KiB/s, so size the deadline from that observed protocol overhead.
    timeout = max(90.0, expected_size * 50.0 / baud + 90.0)
    sender_error: Exception | None = None
    tty_fd = runner.serial.fileno()
    fd_flags = fcntl.fcntl(tty_fd, fcntl.F_GETFL)
    try:
        # pyserial uses Linux's arbitrary-speed support for 460800 even when
        # this host's stty table does not list it.  Hand the configured
        # descriptor directly to sb.  pyserial normally leaves O_NONBLOCK on
        # that descriptor, whereas lrzsz expects blocking terminal reads.
        fcntl.fcntl(tty_fd, fcntl.F_SETFL, fd_flags & ~os.O_NONBLOCK)
        result = subprocess.run(
            [
                "sb",
                "--1k",
                "--binary",
                "--overwrite",
                "--verbose",
                "--verbose",
                str(path),
            ],
            stdin=tty_fd,
            stdout=tty_fd,
            # With -vv this emits live byte/sector progress into the hardware
            # log instead of buffering a multi-minute transfer in memory.
            stderr=None,
            timeout=timeout,
            check=False,
        )
        if result.returncode != 0:
            sender_error = RuntimeError(
                f"YMODEM sender exited with {result.returncode}"
            )
    except subprocess.TimeoutExpired as error:
        sender_error = TimeoutError(
            f"YMODEM sender exceeded its {timeout:.0f}s transfer deadline"
        )
    finally:
        fcntl.fcntl(tty_fd, fcntl.F_SETFL, fd_flags)

    if sender_error is not None:
        # Ensure an interrupted sender cannot strand the board indefinitely in
        # xyzModem receive state.
        runner.write(b"\x18" * 8)
        time.sleep(0.2)
    if switched_baud:
        # A successful receiver still has to flush the downloaded RAM range
        # before it announces the switch back.  Do not guess how long that
        # takes: remain at the transfer baud until the announcement arrives.
        deadline = time.monotonic() + 15
        tail = b""
        switch_back = (
            f"Switch baudrate to {original_baud} bps and press ESC"
        ).encode("ascii")
        while time.monotonic() < deadline:
            tail = (tail + runner.read_and_emit())[-8192:]
            if switch_back in tail:
                break
        else:
            if sender_error is None:
                raise TimeoutError(
                    "U-Boot did not announce its original baud rate"
                )
            # On a receiver-side abort the announcement itself can be lost
            # among the sender's final retries.  The command has nevertheless
            # restored the UART and is waiting for ESC at its original speed.
            runner.serial.baudrate = original_baud
            runner.serial.reset_input_buffer()
            runner.write(b"\x1b")
            switched_baud = False
        # The announcement precedes serial_setbrg() by 50 ms and U-Boot waits
        # another 50 ms before it accepts ESC.
        time.sleep(0.15)
        runner.serial.baudrate = original_baud
        runner.serial.reset_input_buffer()
        runner.write(b"\x1b")
    deadline = time.monotonic() + 10
    tail = b""
    while time.monotonic() < deadline:
        tail = (tail + runner.read_and_emit())[-8192:]
        if b"=> " in tail:
            break
    else:
        raise TimeoutError("U-Boot did not return from YMODEM receive mode")
    if sender_error is not None:
        raise sender_error
    output = runner.command("printenv filesize", 10)
    expected = f"filesize={expected_size:x}".encode("ascii")
    if expected not in output.lower():
        raise RuntimeError(
            f"YMODEM size verification failed; expected {expected.decode()}"
        )


def ymodem_chunked_required(
    runner: SerialRunner,
    address: str,
    path: Path,
    baud: int,
    chunk_size: int,
) -> None:
    """Load a long artifact through independent, contiguous YMODEM runs.

    This BSP2 xyzModem receiver reproducibly NAKs near the 128th 1 KiB packet
    and can then leave receive mode.  Independent sub-128 KiB sessions avoid
    that protocol boundary; bootm still validates the complete image CRC.
    """
    path = path.resolve(strict=True)
    expected_size = path.stat().st_size
    if chunk_size <= 0 or expected_size <= chunk_size:
        ymodem_required(runner, address, path, baud)
        return

    base_address = int(address, 0)
    loaded = 0
    with tempfile.TemporaryDirectory(prefix="moss-kernel-ymodem-") as temp:
        prefix = Path(temp) / "chunk-"
        subprocess.run(
            [
                "split",
                f"--bytes={chunk_size}",
                "--numeric-suffixes=0",
                "--suffix-length=4",
                str(path),
                str(prefix),
            ],
            check=True,
        )
        chunks = sorted(Path(temp).glob("chunk-*"))
        if not chunks:
            raise RuntimeError("split produced no YMODEM kernel chunks")
        print(
            f"\n[bsp2-chain] kernel YMODEM plan: {len(chunks)} chunks, "
            f"chunk_size={chunk_size}, total={expected_size}",
            flush=True,
        )
        for index, chunk in enumerate(chunks, 1):
            print(
                f"\n[bsp2-chain] kernel chunk {index}/{len(chunks)} "
                f"offset=0x{loaded:x}",
                flush=True,
            )
            ymodem_required(
                runner,
                f"0x{base_address + loaded:016x}",
                chunk,
                baud,
            )
            loaded += chunk.stat().st_size

    if loaded != expected_size:
        raise RuntimeError(
            f"chunked YMODEM loaded {loaded}, expected {expected_size} bytes"
        )
    runner.command(f"setenv filesize {expected_size:x}", 5)
    output = runner.command("printenv filesize", 5)
    if f"filesize={expected_size:x}".encode("ascii") not in output.lower():
        raise RuntimeError("failed to publish the complete kernel image size")


def load_required(
    runner: SerialRunner,
    *,
    transport: str,
    address: str,
    filename: str,
    tftp_root: Path,
    ymodem_baud: int,
) -> None:
    if transport == "tftp":
        tftp_required(runner, address, filename)
    else:
        ymodem_required(runner, address, tftp_root / filename, ymodem_baud)


def stop_and_identify_bsp2(runner: SerialRunner, timeout: float) -> None:
    """Span the zero-delay autoboot and require the expected BSP2 banner."""
    stop_event = threading.Event()

    def send_stop_key() -> None:
        while not stop_event.is_set():
            runner.write(b"c")
            time.sleep(0.002)

    sender = threading.Thread(target=send_stop_key, daemon=True)
    sender.start()
    deadline = time.monotonic() + timeout
    tail = b""
    try:
        while time.monotonic() < deadline:
            tail = (tail + runner.read_and_emit())[-65536:]
            if EXPECTED_BSP2_BANNER in tail and b"=>" in tail:
                return
            if (
                b"U-Boot 2022.04-v2.1.0-00349-g4ce7c922" in tail
                and b"=>" in tail
            ):
                raise RuntimeError(
                    "watchdog returned to factory U-Boot before BSP2 prompt"
                )
        raise TimeoutError("BSP2 U-Boot banner and prompt not observed")
    finally:
        stop_event.set()
        sender.join(timeout=1)
        runner.write(b"\x03\r")
        time.sleep(0.1)
        runner.read_and_emit(4096)


def monitor_linux_smoke(runner: SerialRunner, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    tail = b""
    tests_sent = False
    while time.monotonic() < deadline:
        tail = (tail + runner.read_and_emit())[-65536:]
        if not tests_sent and (
            b"Run /bin/sh as init process" in tail
            or b"Starting init: /bin/sh" in tail
        ):
            tests_sent = True
            # Build the result markers in the shell.  Their complete byte
            # strings are therefore absent from terminal input echo.
            runner.write(
                # A bare init=/bin/sh does not initialize login variables.
                # Give rustup the same paths used by the competition root's
                # normal root login instead of letting it fall back to /.rustup.
                b"export HOME=/root USER=root RUSTUP_HOME=/root/.rustup "
                b"CARGO_HOME=/root/.cargo "
                b"PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:"
                b"/usr/sbin:/usr/bin:/sbin:/bin\r"
                # init=/bin/sh bypasses the root filesystem's init system, so
                # mount procfs explicitly before invoking rustup shims.  They
                # resolve themselves through /proc/self/exe and otherwise
                # report a false toolchain failure.
                b"if [ ! -r /proc/self/stat ]; then "
                b"mount -t proc proc /proc; fi; echo proc_rc=$?\r"
                b"printf '__BSP2_%s__\\n' TEST_BEGIN\r"
                b"uname -a\r"
                b"getconf GNU_LIBC_VERSION; echo glibc_rc=$?\r"
                b"/usr/bin/timeout 2 /bin/true; echo timeout_rc=$?\r"
                b"rustc --version; echo rustc_rc=$?\r"
                b"cargo --version; echo cargo_rc=$?\r"
                b"findmnt -n -o SOURCE,FSTYPE,OPTIONS / 2>/dev/null || "
                b"mount | head\r"
                b"printf '__BSP2_%s__\\n' TEST_END\r"
            )
        if tests_sent and b"__BSP2_TEST_END__" in tail:
            required = (
                b"Linux LS2K1000",
                b"GNU_LIBC",
                b"timeout_rc=0",
                b"rustc_rc=0",
                b"cargo_rc=0",
                b"proc_rc=0",
            )
            # The hostname need not be LS2K1000, so accept the version line as
            # long as it identifies the candidate kernel and every ABI smoke
            # command succeeded.
            return (
                b"6.12.0.lsgd" in tail
                and b"glibc 2.41" in tail
                and b"/dev/sda2" in tail
                and all(item in tail for item in required[2:])
            )
    return False


def monitor_linux_workload(
    runner: SerialRunner,
    workload: str,
    timeout: float,
    swap_device: str,
    swap_file: str,
) -> tuple[bool, bool]:
    """Run real contest workloads while keeping the serial shell responsive.

    Returns ``(passed, linux_active)``.  ``linux_active`` is false when the
    hardware watchdog has already restored the factory boot path.
    """
    deadline = time.monotonic() + timeout if timeout > 0 else None
    boot_tail = b""
    line_buffer = b""
    tests_sent = False
    cagent_results: dict[str, str] = {}
    evidence = {
        "root_rw": False,
        "proc": False,
        "loopback": False,
        "watchdog_ping": False,
        "swap": False,
        "toolchain": False,
        "minibuild": False,
        "build": False,
        "end": False,
    }

    run_cagent = workload in ("cagent", "all")
    run_buildstorm = workload in ("buildstorm", "all")
    if not swap_file.startswith("/"):
        raise ValueError("--swap-file must be an absolute path")
    swap_device_shell = shlex.quote(swap_device)
    swap_path_shell = shlex.quote(f"/mnt/linux-root{swap_file}")

    while deadline is None or time.monotonic() < deadline:
        data = runner.read_and_emit()
        if not data:
            continue
        boot_tail = (boot_tail + data)[-65536:]

        if (
            not tests_sent
            and b"can't access tty; job control turned off" in boot_tail
        ):
            tests_sent = True
            setup_commands = [
                (
                    "export HOME=/root USER=root "
                    "RUSTUP_HOME=/root/.rustup CARGO_HOME=/root/.cargo "
                    "RUSTUP_TOOLCHAIN=nightly-2026-05-28 "
                    "CARGO_NET_OFFLINE=true "
                    "PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:"
                    "/usr/sbin:/usr/bin:/sbin:/bin",
                    5,
                ),
                ("mount -o remount,rw /; echo root_rw_rc=$?", 20),
                (
                    "if [ ! -r /proc/self/stat ]; then "
                    "mount -t proc proc /proc; fi; echo proc_rc=$?",
                    20,
                ),
                (
                    "if ! grep -q ' /sys ' /proc/mounts; then "
                    "mount -t sysfs sysfs /sys; fi; echo sysfs_rc=$?",
                    20,
                ),
                (
                    "mkdir -p /dev/pts; "
                    "if ! grep -q ' /dev/pts ' /proc/mounts; then "
                    "mount -t devpts devpts /dev/pts; fi; "
                    "echo devpts_rc=$?",
                    20,
                ),
                ("ip link set lo up; echo loopback_rc=$?", 20),
                ("mkdir -p /mnt/linux-root", 5),
                (
                    f"mount -t ext4 -o rw {swap_device_shell} "
                    "/mnt/linux-root; echo swap_mount_rc=$?",
                    180,
                ),
                (
                    f"swapon {swap_path_shell}; echo swapon_rc=$?",
                    30,
                ),
                (
                    f"grep -F {swap_path_shell} /proc/swaps >/dev/null; "
                    "echo swap_active_rc=$?",
                    20,
                ),
                # Prove /dev/mem access before relying on the background
                # pinger.  If the kernel hard-locks, the pinger also stops and
                # the already armed 30-second hardware watchdog resets it.
                (
                    "/glibc/busybox devmem 0x1fe27034 32 1 >/dev/null; "
                    "echo wdt_ping_rc=$?",
                    20,
                ),
                (
                    "(while :; do /glibc/busybox devmem 0x1fe27034 32 1 "
                    ">/dev/null 2>&1; /glibc/busybox sleep 5; done) "
                    ">/tmp/bsp2-wdt-pinger.log 2>&1 & "
                    "echo $! >/tmp/bsp2-wdt-pinger.pid",
                    20,
                ),
                ("printf '__BSP2_%s__\\n' WORKLOAD_BEGIN", 5),
            ]
            setup_output = b""
            for setup_command, setup_timeout in setup_commands:
                setup_output += runner.linux_command(
                    setup_command, setup_timeout
                )

            evidence["root_rw"] = b"root_rw_rc=0" in setup_output
            evidence["proc"] = b"proc_rc=0" in setup_output
            evidence["loopback"] = b"loopback_rc=0" in setup_output
            evidence["watchdog_ping"] = b"wdt_ping_rc=0" in setup_output
            evidence["swap"] = b"swap_active_rc=0" in setup_output
            if not all(
                evidence[key]
                for key in (
                    "root_rw",
                    "proc",
                    "loopback",
                    "watchdog_ping",
                    "swap",
                )
            ):
                print(
                    "\n[bsp2-chain] workload setup=FAIL; refusing to "
                    "start cagent/buildstorm",
                    flush=True,
                )
                return False, True

            workload_commands = []
            if run_cagent:
                workload_commands.append(
                    "./cagent_testcode.sh; echo cagent_script_rc=$?"
                )
            if run_buildstorm:
                workload_commands.append(
                    "./buildstorm_testcode.sh; echo buildstorm_script_rc=$?"
                )
            workload_commands.append(
                "printf '__BSP2_%s__\\n' WORKLOAD_END"
            )
            workload_command = (
                "(cd /glibc; "
                + "; ".join(workload_commands)
                + ") & echo $! >/tmp/bsp2-workload.pid"
            )
            runner.write(workload_command.encode("ascii") + b"\r")

        if tests_sent:
            line_buffer += data.replace(b"\r", b"\n")
            lines = line_buffer.split(b"\n")
            line_buffer = lines.pop()
            for raw_line in lines:
                line = raw_line.strip()
                match = re.search(
                    rb"testcase cagent ([^ ]+) (pass|reject) [0-9]+", line
                )
                if match:
                    cagent_results[match.group(1).decode("ascii")] = (
                        match.group(2).decode("ascii")
                    )
                evidence["root_rw"] |= b"root_rw_rc=0" in line
                evidence["proc"] |= b"proc_rc=0" in line
                evidence["loopback"] |= b"loopback_rc=0" in line
                evidence["watchdog_ping"] |= b"wdt_ping_rc=0" in line
                evidence["swap"] |= b"swap_active_rc=0" in line
                evidence["toolchain"] |= b"BUILDSTORM_TOOLCHAIN ok" in line
                evidence["minibuild"] |= b"BUILDSTORM_MINIBUILD ok" in line
                evidence["build"] |= (
                    b"BUILDSTORM_COMPILE mode=multi ok=true" in line
                )
                evidence["end"] |= b"__BSP2_WORKLOAD_END__" in line

        if tests_sent and evidence["end"]:
            cagent_passed = (
                not run_cagent
                or (
                    len(cagent_results) == 10
                    and all(
                        result == "pass"
                        for result in cagent_results.values()
                    )
                )
            )
            buildstorm_passed = (
                not run_buildstorm
                or (
                    evidence["toolchain"]
                    and evidence["minibuild"]
                    and evidence["build"]
                )
            )
            environment_passed = all(
                evidence[key]
                for key in (
                    "root_rw",
                    "proc",
                    "loopback",
                    "watchdog_ping",
                    "swap",
                )
            )
            passed = (
                environment_passed and cagent_passed and buildstorm_passed
            )
            pass_count = sum(
                result == "pass" for result in cagent_results.values()
            )
            reject_count = sum(
                result == "reject" for result in cagent_results.values()
            )
            print(
                "\n[bsp2-chain] workload summary "
                f"cagent={pass_count} pass/{reject_count} reject "
                f"toolchain={'PASS' if evidence['toolchain'] else 'N/A/FAIL'} "
                f"minibuild={'PASS' if evidence['minibuild'] else 'N/A/FAIL'} "
                f"build={'PASS' if evidence['build'] else 'N/A/FAIL'}",
                flush=True,
            )
            return passed, True

        if tests_sent and (
            b"U-Boot 2022.04-v2.1.0-00349-g4ce7c922" in boot_tail
        ):
            print(
                "\n[bsp2-chain] hardware watchdog restored factory U-Boot "
                "before workload completion",
                flush=True,
            )
            return False, False

    print("\n[bsp2-chain] host-side workload deadline reached", flush=True)
    return False, True


def enter_bsp2_from_factory(
    runner: SerialRunner, args: argparse.Namespace
) -> None:
    runner.wait_for_linux_shell(args.linux_timeout)
    # The factory udev daemon auto-mounts sda2 in its private mount namespace.
    # A root-namespace `umount /dev/sda2` cannot see that mount.  Stop udev only
    # for the few seconds before reboot so the namespace is destroyed and the
    # competition filesystem is clean before the read-only candidate boot.
    runner.linux_command(
        "systemctl stop systemd-udevd-control.socket "
        "systemd-udevd-kernel.socket systemd-udevd.service"
    )
    runner.linux_command("umount /dev/sda2 2>/dev/null || true; sync")
    print("\n[bsp2-chain] rebooting into RAM-only BSP2 test", flush=True)
    runner.write(b"/sbin/reboot -f\r")
    runner.stop_uboot_autoboot()

    factory_commands = [
        ("setenv ipaddr 192.168.88.2", 5),
        ("setenv serverip 192.168.88.1", 5),
        ("setenv netmask 255.255.255.0", 5),
        ("setenv ethact ethernet@40040000", 5),
        # Keep bootm in load-only mode.  A legacy image stores only a 32-bit
        # entry address, so invoke the 64-bit cached entry explicitly below.
        ("setenv autostart no", 5),
    ]
    for command, command_timeout in factory_commands:
        runner.command(command, command_timeout)
    load_required(
        runner,
        transport=args.transport,
        address=UBOOT_STAGING_ADDRESS,
        filename=args.uboot,
        tftp_root=args.tftp_root,
        ymodem_baud=args.ymodem_baud,
    )
    runner.command(f"crc32 {UBOOT_STAGING_ADDRESS} ${{filesize}}", 10)
    arm_watchdog(runner)

    # The factory U-Boot's low 256 MiB contains its framebuffer and runtime
    # reservations.  The wrapper is downloaded to proven high memory; bootm
    # verifies and copies it to its non-overlapping link address, flushes
    # caches and returns.  The 64-bit go command then enters it.
    bootm_output = runner.command(f"bootm {UBOOT_STAGING_ADDRESS}", 15)
    if b"Loading Standalone Program" not in bootm_output:
        raise RuntimeError("factory bootm did not load the BSP2 payload")

    go_uboot = f"go {UBOOT_ENTRY_ADDRESS}"
    print(f"\n[bsp2-chain] >>> {go_uboot}", flush=True)
    runner.write(go_uboot.encode("ascii") + b"\r")
    try:
        stop_and_identify_bsp2(runner, args.handoff_timeout)
    except Exception:
        # The watchdog may already have returned to the factory prompt.  A
        # reset releases that prompt into its untouched normal boot path.
        runner.write(b"reset\r")
        runner.monitor(args.recovery_monitor_seconds)
        raise
    print("\n[bsp2-chain] BSP2 U-Boot RAM handoff=PASS", flush=True)


def require_current_bsp2(runner: SerialRunner) -> None:
    """Prove that resume mode is attached to the RAM-only BSP2 prompt."""
    runner.write(b"\x03\r")
    time.sleep(0.1)
    runner.serial.reset_input_buffer()
    output = runner.command("version", 5)
    if EXPECTED_BSP2_BANNER not in output:
        raise RuntimeError("--resume-bsp2 did not find the expected BSP2 U-Boot")
    print("\n[bsp2-chain] existing BSP2 U-Boot prompt=PASS", flush=True)


def boot_and_test_linux(
    runner: SerialRunner, args: argparse.Namespace
) -> None:
    # The handoff watchdog is irrelevant during a multi-minute serial transfer;
    # stop it now and re-arm it immediately before booting Linux.
    if args.transport == "ymodem":
        runner.command("mw.l 0x1fe27030 0", 5)

    root_mode = "ro" if args.linux_test == "smoke" else "rw"
    bootargs = (
        "earlycon console=ttyS0,115200 root=/dev/sda2 "
        f"rootfstype=ext4 {root_mode} rootwait init=/bin/sh panic=10"
    )
    bsp2_commands = [
        ("bdinfo", 5),
        ("setenv ipaddr 192.168.88.2", 5),
        ("setenv serverip 192.168.88.1", 5),
        ("setenv netmask 255.255.255.0", 5),
        ("setenv ethact ethernet@40040000", 5),
        (f"setenv bootargs {bootargs}", 5),
    ]
    for command, command_timeout in bsp2_commands:
        runner.command(command, command_timeout)
    if args.transport == "ymodem":
        ymodem_chunked_required(
            runner,
            KERNEL_ADDRESS,
            args.tftp_root / args.kernel,
            args.ymodem_baud,
            args.kernel_ymodem_chunk_size,
        )
    else:
        load_required(
            runner,
            transport=args.transport,
            address=KERNEL_ADDRESS,
            filename=args.kernel,
            tftp_root=args.tftp_root,
            ymodem_baud=args.ymodem_baud,
        )
    arm_watchdog(runner)

    bootm = f"bootm {KERNEL_ADDRESS}"
    print(f"\n[bsp2-chain] >>> {bootm}", flush=True)
    runner.write(bootm.encode("ascii") + b"\r")
    linux_active = True
    if args.linux_test == "smoke":
        passed = monitor_linux_smoke(runner, 35)
        result_name = "smoke"
    else:
        passed, linux_active = monitor_linux_workload(
            runner,
            args.linux_test,
            args.workload_timeout,
            args.swap_device,
            args.swap_file,
        )
        result_name = f"workload {args.linux_test}"
    print(
        f"\n[bsp2-chain] Linux 6.12 + competition root {result_name}="
        f"{'PASS' if passed else 'FAIL'}",
        flush=True,
    )
    if linux_active:
        # The competition image intentionally has no reboot executable.  Do
        # not jump straight from sync(2) to Magic SysRq here: buildstorm can
        # create and unlink thousands of temporary files, and an immediate
        # reset left those inodes on ext4's orphan list even though their data
        # had reached disk.  Stop every remaining writer and remount the sda2
        # root read-only before asking the kernel to restart.  The already
        # armed hardware watchdog remains the fallback if cleanup stalls.
        if args.linux_test != "smoke":
            swap_path = shlex.quote(
                f"/mnt/linux-root{args.swap_file}"
            )
            runner.linux_command(
                f"/usr/bin/timeout 120 swapoff {swap_path}; "
                "echo swapoff_rc=$?; sync; "
                "umount /mnt/linux-root; echo swap_umount_rc=$?",
                timeout=150,
            )
        root_ro_output = runner.linux_command(
            "pinger_pid=$(cat /tmp/bsp2-wdt-pinger.pid 2>/dev/null); "
            "kill $pinger_pid 2>/dev/null; "
            "wait $pinger_pid 2>/dev/null || true; "
            "sync; mount -o remount,ro /; "
            "echo root_ro_rc=$?",
            timeout=25,
        )
        if b"root_ro_rc=0" in root_ro_output:
            runner.write(b"echo b > /proc/sysrq-trigger\r")
        else:
            print(
                "\n[bsp2-chain] root read-only remount=FAIL; "
                "waiting for the hardware watchdog instead of issuing an "
                "unsafe immediate reboot",
                flush=True,
            )
            passed = False
    runner.monitor(args.recovery_monitor_seconds)
    if not passed:
        raise SystemExit(1)


def main() -> None:
    args = parse_args()
    runner = SerialRunner(args.device)
    try:
        if args.resume_bsp2:
            require_current_bsp2(runner)
        else:
            enter_bsp2_from_factory(runner, args)
        boot_and_test_linux(runner, args)
    finally:
        runner.close()


if __name__ == "__main__":
    main()
