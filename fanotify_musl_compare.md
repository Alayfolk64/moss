# Fanotify musl 对比

## 对比口径

- Ubuntu 日志：`/Users/wuxun/os/fanotify01_23.txt`
- StarryOS 日志：`/Users/wuxun/os/StarryOS/output_la.txt`
- StarryOS 只取 `ltp-musl` 段：
  - 起点：`#### OS COMP TEST GROUP START ltp-musl ####`
  - 终点：`#### OS COMP TEST GROUP END ltp-musl ####`
- `p/f/b/s/w` 表示 `passed/failed/broken/skipped/warnings`。
- `fs(U/S)` 表示 Ubuntu / StarryOS 跑到的文件系统数量。

两边 all-filesystems 矩阵数量一致，都是 9 个：

```text
ext2 ext3 ext4 xfs btrfs vfat exfat ntfs tmpfs
```

## 逐项对比

| case | fs(U/S) | Ubuntu p/f/b/s/w | StarryOS musl p/f/b/s/w | Delta pass | Delta skip | Delta broken | 主要差异 |
|---|---:|---:|---:|---:|---:|---:|---|
| fanotify01 | 9/9 | 650/0/0/4/0 | 702/0/0/0/0 | +52 | -4 | 0 | StarryOS 对 exfat/ntfs 没跳过部分 mark |
| fanotify02 | 0/0 | 8/0/0/0/0 | 8/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify03 | 0/0 | 21/0/0/0/0 | 21/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify04 | 0/0 | 9/0/0/0/0 | 9/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify05 | 0/0 | 2/0/0/0/0 | 2/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify06 | 0/0 | 18/0/0/0/0 | 9/0/0/1/0 | -9 | +1 | 0 | StarryOS 缺 overlayfs |
| fanotify07 | 0/0 | 2/0/0/0/0 | 2/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify08 | 0/0 | 2/0/0/0/0 | 2/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify09 | 0/0 | 76/0/0/0/0 | 76/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify10 | 0/0 | 1047/0/0/3/0 | 1047/0/0/3/0 | 0 | 0 | 0 | 一致 |
| fanotify11 | 0/0 | 2/0/0/0/0 | 2/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify12 | 0/0 | 10/0/0/0/0 | 10/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify13 | 9/9 | 293/0/0/152/0 | 135/0/0/216/0 | -158 | +64 | 0 | StarryOS overlayfs 变体全跳过 |
| fanotify14 | 9/9 | 497/0/0/0/0 | 513/0/0/0/0 | +16 | 0 | 0 | fs 能力探测/兼容 fs 行为不同 |
| fanotify15 | 9/9 | 80/0/0/2/0 | 90/0/0/0/0 | +10 | -2 | 0 | StarryOS exfat/ntfs 没跳过 |
| fanotify16 | 9/9 | 1228/0/0/16/0 | 1386/0/0/0/0 | +158 | -16 | 0 | StarryOS exfat/ntfs 没跳过 |
| fanotify17 | 0/0 | 4/0/0/0/0 | 1/0/0/3/0 | -3 | +3 | 0 | StarryOS 不支持 user namespace 内 fanotify |
| fanotify18 | 0/0 | 9/0/0/0/0 | 9/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify19 | 0/0 | 16/0/0/0/0 | 16/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify20 | 9/9 | 18/0/0/0/0 | 18/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify21 | 9/9 | 18/0/0/0/0 | 18/0/0/0/0 | 0 | 0 | 0 | 一致 |
| fanotify22 | 0/0 | 4/0/0/0/0 | 0/0/0/1/0 | -4 | +1 | 0 | StarryOS 环境缺 `debugfs` |
| fanotify23 | 0/0 | 6/0/0/0/0 | 2/0/1/0/0 | -4 | 0 | +1 | StarryOS 没收到 `FAN_ATTRIB` 事件 |

## 总计

| 日志 | passed | failed | broken | skipped | warnings |
|---|---:|---:|---:|---:|---:|
| Ubuntu musl | 4020 | 0 | 0 | 177 | 0 |
| StarryOS LA musl | 4078 | 0 | 1 | 224 | 0 |
| Delta | +58 | 0 | +1 | +47 | 0 |

## 结论

- 分数不同不是因为文件系统数量不同；两边 all-filesystems 矩阵都是 9 个文件系统。
- StarryOS pass 数更高的一部分原因是 compat fs 对 exfat/ntfs 没有像 Ubuntu/FUSE 那样跳过部分 fanotify mark。
- StarryOS skipped 更多主要来自：
  - `fanotify06` / `fanotify13`：缺 overlayfs。
  - `fanotify17`：不支持 user namespace 内 fanotify。
  - `fanotify22`：测试环境缺 `debugfs`。
- StarryOS 目前唯一 broken 是 `fanotify23`，原因是 `chmod()` 之后没有收到预期的 `FAN_ATTRIB` 事件。
