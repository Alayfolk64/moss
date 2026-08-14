#!/usr/bin/env python3

#############
#
# 使用方法: python3 scripts/update_vendor_checksum.py vendor/axfs
#
############

import hashlib
import json
import sys
from pathlib import Path


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()

    with path.open("rb") as f:
        while chunk := f.read(1024 * 1024):
            h.update(chunk)

    return h.hexdigest()


if len(sys.argv) != 2:
    print(f"Usage: {sys.argv[0]} vendor/<crate>")
    sys.exit(1)


# scripts/update_vendor_checksum.py
#          ↑
#          script_dir
#
# 仓库根目录就是 scripts 的父目录
script_dir = Path(__file__).resolve().parent
repo_root = script_dir.parent

crate = Path(sys.argv[1])

if not crate.is_absolute():
    crate = repo_root / crate

crate = crate.resolve()

checksum_path = crate / ".cargo-checksum.json"

if not crate.is_dir():
    print(f"error: crate directory does not exist: {crate}")
    sys.exit(1)

if not checksum_path.is_file():
    print(f"error: checksum file does not exist: {checksum_path}")
    sys.exit(1)


# 先读取旧文件，主要是为了保留 package 字段
data = json.loads(checksum_path.read_text())

new_files = {}


# 遍历整个 vendor crate
for path in sorted(crate.rglob("*")):
    if not path.is_file():
        continue

    rel = path.relative_to(crate)

    # .cargo-checksum.json 不能把自己算进去，
    # 否则每次更新自己都会导致自己的 hash 再变化
    if rel == Path(".cargo-checksum.json"):
        continue

    # 这些本地生成目录也不应该算进 vendored crate
    if ".git" in rel.parts:
        continue

    if "target" in rel.parts:
        continue

    # Cargo checksum 中统一使用 / 作为路径分隔符
    rel_str = rel.as_posix()

    new_files[rel_str] = sha256_file(path)


old_files = data.get("files", {})

added = sorted(set(new_files) - set(old_files))
removed = sorted(set(old_files) - set(new_files))
modified = sorted(
    path
    for path in set(new_files) & set(old_files)
    if new_files[path] != old_files[path]
)


# 替换整个 files 表
# package 字段不会动
data["files"] = new_files

checksum_path.write_text(
    json.dumps(data, separators=(",", ":")) + "\n"
)


print(f"updated {checksum_path}")

if added:
    print("\nAdded:")
    for path in added:
        print(f"  + {path}")

if modified:
    print("\nModified:")
    for path in modified:
        print(f"  M {path}")

if removed:
    print("\nRemoved:")
    for path in removed:
        print(f"  - {path}")

if not added and not modified and not removed:
    print("No checksum changes.")