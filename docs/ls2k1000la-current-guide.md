# 龙芯 2K1000LA 当前方案与使用指南

更新日期：2026-08-12

本文只记录当前可继续使用的方案。完整实验过程和旧镜像证据保留在
[调试状态报告](ls2k1000la-debugging-status-20260808.md) 与
[Linux 6.12 验收报告](ls2k1000la-linux612-usage-report-20260811.md) 中。

## 1. 已确认的硬件与磁盘基线

- 星云版为 2 核 LA264、1 GiB DDR3；原厂固件显示的 2 GiB 不是可靠的物理探测结果。
- `/dev/sda1` 约 17.816 GiB，保留原厂 Linux、旧内核和 2 GiB `/swapfile-moss`。
- `/dev/sda2` 严格 12 GiB，保存比赛根文件系统，也是 MOSS 的根分区。
- 原厂 Linux 5.10 和旧内核均未删除。Linux 6.12 只完成过 RAM-chain 验收，没有替换默认内核。
- Linux 6.12 已实测 cagent 10/10；buildstorm 输出 133 条 `Compiling`，43 分 54 秒完成，
  最终为 `ok=true elapsed_s=2663.92 cores=2 bytes=1714568 arch=loongarch64`。

## 2. 当前两个 MOSS 镜像

| 模式 | 构建命令 | 产物 | 行为 |
|---|---|---|---|
| 生产 | `make kernel-la-2k1000` | `moss-2k1000la.bin` | 自动运行比赛脚本，启用硬件 watchdog |
| 交互 | `make kernel-la-2k1000-shell` | `moss-2k1000la-shell.bin` | 进入 BusyBox shell，并在早期启动时关闭继承自 U-Boot 的 watchdog |

生产模式使用 30 秒硬件 watchdog。内核每秒检查一次前进状态，用户任务集合发生变化、明确的
启动/退出里程碑或用户态 CPU 工作量达到单核 10% 时，都会把 20 分钟无进展租约向后顺延。
只有连续 20 分钟没有这些进展才永久停止喂狗，随后约 30 秒由硬件复位。panic/assert 则直接
走 DT 的 reboot，不等待租约。

交互镜像没有 watchdog 自动复位，适合手工输入和写文件。交互 shell 中 `Ctrl+C` 只向前台
进程组发送 `SIGINT`，不会直接 reboot；需要重启时显式执行 `reboot`。

## 3. 新的精简工具

当前板级流程只保留两个专用工具：

1. `scripts/check_ls2k1000_static.sh`
   - 使用 `dtc`、`fdtget` 验证双核、AHCI、GMAC、RTC、watchdog 和 syscon 的 DT 合同；
   - 使用宿主机 `rustc --test` 运行 15 组不访问 MMIO 的解析/状态机测试；
   - 使用 `llvm-readelf`、`llvm-objdump` 检查 LoongArch 入口、上下文切换、DTB 访问和
     `.dma_uncached` 段；
   - 实际构建并检查生产、交互两个镜像，不再用大量源码正则猜测实现顺序。
2. `scripts/run_ls2k1000_serial_tftp.py`
   - 使用 Python `pyserial`，直接操作 `/dev/ttyUSB0`，不需要浏览器；
   - 从原厂 Linux 安全卸载 sda2、解析 `/swapfile-moss` 的物理 extent，然后重启并连续发送
     U-Boot 停止键；
   - 自动设置网络、DT bootargs、swap extent、AHCI/GMAC 属性，TFTP 加载镜像并采集串口日志；
   - 在 `go` 前安装 30 秒 U-Boot watchdog，覆盖 TFTP 后到 MOSS 早期初始化的空窗。交互镜像
     进入 MOSS 后会主动关闭它。

此前的 BSP2 chain、rescue initramfs、旧 Linux uImage runner、分区写盘脚本均属于一次性实验
工具，不再作为当前生产流程入口；其结果和命令仍可从两份历史报告追溯。

## 4. 构建、验收与上板

先在宿主机完成静态验收：

```bash
./scripts/check_ls2k1000_static.sh
```

生产模式部署：

```bash
make kernel-la-2k1000
sudo install -o tftp -g tftp -m 0644 \
  moss-2k1000la.bin /srv/tftp/moss-current.bin
python3 scripts/run_ls2k1000_serial_tftp.py moss-current.bin \
  --monitor-seconds 180 --log /tmp/moss-ls2k1000la.log
```

交互模式部署：

```bash
make kernel-la-2k1000-shell
sudo install -o tftp -g tftp -m 0644 \
  moss-2k1000la-shell.bin /srv/tftp/moss-shell.bin
python3 scripts/run_ls2k1000_serial_tftp.py moss-shell.bin \
  --monitor-seconds 180 --log /tmp/moss-ls2k1000la-shell.log
```

runner 的监视时间只决定主机脚本何时退出，不会给板上 workload 设置截止时间。脚本退出后可用
任意 115200 8N1 串口终端重新打开 `/dev/ttyUSB0`，继续操作交互 shell。

## 5. 当前验收边界

宿主机静态检查能证明两个镜像可构建、DT 合同和纯状态机测试通过，但不能代替实板数据路径。
最终完成标准仍包括：MOSS 在双核/1 GiB 下完成 cagent 与完整 buildstorm；AHCI 无命令超时或
ext4 错误；GMAC0 完成真实双向 TX/RX；生产 watchdog 能在连续 20 分钟无用户进展后复位，且
正常长编译会持续顺延租约；交互镜像保持可输入且不自动复位。
