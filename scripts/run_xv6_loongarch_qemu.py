#!/usr/bin/env python3
"""Boot an LLVM-built LoongArch xv6 image and exercise its ordinary shell."""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

from run_xv6_riscv_qemu import Xv6Runner


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--qemu-runenv", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument(
        "--docker-image", default="moss-xv6-loongarch-qemu:ubuntu20.04"
    )
    parser.add_argument("--hard-deadline", type=float, default=120)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    kernel = args.kernel.resolve()
    runenv = args.qemu_runenv.resolve()
    if args.hard_deadline <= 0:
        raise SystemExit("--hard-deadline must be positive")
    if not kernel.is_file():
        raise SystemExit(f"missing xv6 kernel: {kernel}")
    for name in (
        "qemu-system-loongarch64",
        "loongarch_bios_0310.bin",
        "busybox-rootfs.img",
        "efi-virtio.rom",
    ):
        if not (runenv / name).is_file():
            raise SystemExit(f"missing LoongArch QEMU runenv file: {runenv / name}")

    container_name = f"moss-xv6-la-{os.getpid()}"
    command = [
        "docker",
        "run",
        "--rm",
        "--interactive",
        "--name",
        container_name,
        "--workdir",
        "/runenv",
        "--volume",
        f"{runenv}:/runenv:ro",
        "--volume",
        f"{kernel}:/kernel:ro",
        args.docker_image,
        "./qemu-system-loongarch64",
        "-m",
        "1G",
        "-smp",
        "1",
        "-bios",
        "./loongarch_bios_0310.bin",
        "-kernel",
        "/kernel",
        "-initrd",
        "./busybox-rootfs.img",
        "-append",
        "root=/dev/ram console=ttyS0,115200 rdinit=/init",
        "-vga",
        "none",
        "-nographic",
    ]
    runner = Xv6Runner(command, args.log, args.hard_deadline)
    try:
        runner.expect(b"init: starting sh", timeout=90)
        runner.expect(b"$ ")
        runner.emit("[xv6-qemu] PASS booted LLVM LoongArch xv6 to its shell")
        runner.command("mkdir accept")
        runner.command("echo XV6_LOONGARCH_LLVM_OK > accept/boot-marker")
        runner.command("cat accept/boot-marker", b"XV6_LOONGARCH_LLVM_OK")
        runner.command("echo alpha beta gamma > accept/data")
        runner.command("cat accept/data", b"alpha beta gamma")
        runner.command("grep beta accept/data", b"alpha beta gamma")
        runner.command("wc accept/data", b"1 3 17 accept/data")
        runner.command("ls accept", b"boot-marker")
        runner.command("rm accept/data")
        runner.emit("[xv6-qemu] ALL XV6 LOONGARCH LLVM BOOT TESTS PASSED")
        return 0
    finally:
        runner.close()
        subprocess.run(
            ["docker", "rm", "--force", container_name],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=15,
        )


if __name__ == "__main__":
    raise SystemExit(main())
