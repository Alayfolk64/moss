# 龙芯 2K1000LA 星云板 Linux 6.12 使用与验收报告

日期：2026-08-11

## 1. 当前结论

本报告记录的是已经真机跑通的 **Linux 6.12** 流程。板卡为 2 核 LA264、1 GiB DDR3；原厂
U-Boot 和 Linux 5.10 显示的 2 GiB 来自错误的编译期内存表，不是物理探测结果。

已在同一块板、同一份 sda2 比赛环境上完成以下验收：

- cagent：10 pass / 0 reject；
- buildstorm toolchain、minibuild：PASS；
- buildstorm release：133 条 `Compiling`，43 分 54 秒完成；
- 最终结果：`ok=true elapsed_s=2663.92 cores=2 bytes=1714568 arch=loongarch64`；
- 工作负载结束后 swap 可正常停用，两个文件系统可安全卸载/切成只读；
- 重启后 sda2 `e2fsck -fn` 返回 0。

当前稳定链路仍是一次性 RAM-chain 验证，板上默认启动仍为原厂 Linux 5.10。旧内核没有被覆盖或
删除，Linux 6.12 尚未写成默认启动项。

## 2. 硬件、磁盘和内存基线

32 GB SATA 盘采用两个 1 MiB 对齐分区：

| 设备 | sector 范围 | 容量 | 用途 |
|---|---:|---:|---|
| `/dev/sda1` | 2,048–37,365,759 | 约 17.816 GiB | 原厂 Linux、旧内核、恢复文件、swapfile |
| `/dev/sda2` | 37,365,760–62,531,583 | 严格 12 GiB | MOSS/Linux 6.12 共用的比赛根文件系统 |

当前保留的交换空间不是第三个 swap 分区，而是 sda1 上的：

```text
/swapfile-moss    2 GiB    mode 0600
```

Linux 6.12 启动 sda2 后，将 sda1 挂到 `/mnt/linux-root`，再启用
`/mnt/linux-root/swapfile-moss`。因此保留干净双分区的同时仍有 2 GiB 交换空间。

正确的内存证据为：

| 层级 | 真机输出 |
|---|---|
| BSP2 U-Boot v8 | `DRAM: 1024 MiB` |
| BSP2 `bdinfo` | 低 256 MiB + 高 768 MiB |
| Linux 6.12 early memory | `799760K/1030144K available` |
| Linux 6.12 `/proc/meminfo` | `MemTotal: 835920 kB` |
| swap | `SwapTotal: 2097136 kB` |

## 3. 当前启动链

启动链不写 SPI，分为三段：

1. 原厂 U-Boot 从板载 SPI 启动；
2. 原厂 U-Boot 将只读 BSP2 wrapper 加载到 `0x9000000098000000`，legacy header 再把
   U-Boot proper 放到 `0x0c000000` 执行；
3. BSP2 从 TFTP 加载启用 LSX 的 Linux 6.12 uImage，以
   `root=/dev/sda2 rootfstype=ext4 rootwait init=/bin/sh` 进入比赛根。

测试 runner 在两个固件交接点都安装 30 秒 LS2K 硬件 watchdog。Linux 用户态就绪后由独立 pinger
每 5 秒重载 watchdog；因此长时间 `rustc` 计算不会被误杀，而内核、IRQ 或调度器真正停止运行时，
板子会在约 30 秒内自动复位回原厂启动链。

正式 buildstorm 的复跑入口为：

```bash
python3 scripts/run_ls2k1000_bsp2_chain.py \
  ls2k1000-dp-bsp2-u-boot-chain-v8-c000000-readonly.img \
  linux-6.12.0-lsgd-moss-lsx-uImage \
  --transport tftp \
  --linux-test buildstorm \
  --workload-timeout 0 \
  --swap-device /dev/sda1 \
  --swap-file /swapfile-moss \
  --recovery-monitor-seconds 20
```

`--workload-timeout 0` 表示主机 runner 不对合法长编译设置截止时间，硬件 watchdog 仍覆盖真正的
系统卡死。

## 4. Linux 6.12 用户态准备流程

runner 进入 `init=/bin/sh` 后按以下顺序准备环境：

1. 根文件系统 remount 为读写；
2. 挂载 `/proc`、`/sys`、`/dev/pts`，启用 loopback；
3. 设置 `/root/.rustup`、`/root/.cargo`、`nightly-2026-05-28` 和离线 Cargo 环境；
4. 将 `/dev/sda1` 挂到 `/mnt/linux-root`；
5. 执行 `swapon /mnt/linux-root/swapfile-moss`，并要求 `swap_active_rc=0`；
6. 验证 direct watchdog reload 后启动 5 秒 pinger；
7. 在 `/glibc` 中执行原始 `cagent_testcode.sh` 或 `buildstorm_testcode.sh`。

现有比赛根自身没有 `/init`、`/sbin/init` 或 `/etc/inittab`，所以 `init=/bin/sh` 只是测试夹具。
若以后改为磁盘自主启动，必须先安装专用 PID 1，不能简单删除 bootargs 中的 `init=`。

## 5. 为什么必须保留 swap

无 swap 对照使用同一个 1 GiB 板卡、同一个 6.12 内核和同一个比赛根，在正式编译 599.76 秒后
被 OOM killer 终止：

```text
Out of memory: Killed process 400 (rustc)
total-vm:1194272kB anon-rss:721392kB
BUILDSTORM_COMPILE mode=multi ok=false rc=1 elapsed_s=599.76
```

启用 2 GiB swap 后，观测到的最大交换使用量约 197 MiB：

```text
MemAvailable: 142624 kB
SwapTotal:   2097136 kB
SwapFree:    1895408 kB
```

这说明 swap 不是形式配置，而是补足 1 GiB 物理内存峰值的必要条件。后段再次采样时
`MemAvailable: 645664 kB`、`SwapFree: 2041552 kB`，表明已经换出的冷匿名页没有妨碍后续编译。

## 6. 完整 cagent/buildstorm 结果

正式日志中的关键验收值为：

```text
BUILDSTORM_TOOLCHAIN ok
BUILDSTORM_MINIBUILD ok
Finished `release` profile [optimized] target(s) in 43m 54s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=2663.92 cores=2 bytes=1714568 arch=loongarch64
swapoff_rc=0
swap_umount_rc=0
```

正式 `Compiling` 输出共 133 条。长时间没有新 crate 文本期间，现场 `ps` 显示 `rustc(core)` 的 CPU
时间几乎等于墙钟时间，证明它在持续计算，不是 AHCI、调度器或 swap 卡死。

原始证据：

- [cagent 真机日志](../../hardware-linux-ls2k1000la-20260811-linux612-1g-swap-cagent-buildstorm.log)
- [buildstorm 完整真机日志](../../hardware-linux-ls2k1000la-20260811-linux612-1g-swap-buildstorm-v3.log)
- [无 swap OOM 对照](../../hardware-linux-ls2k1000la-20260811-linux612-cagent-buildstorm-tftp.log)

## 7. 正确退出与文件系统保护

不能在 buildstorm 后只执行 `sync` 再立即写 SysRq reboot。实测这种做法会让 sda2 留下数千个
ext4 orphan 条目，下一次 `e2fsck -fn` 报 `corrupted orphan linked list` 并返回 4。

正确退出顺序为：

1. `swapoff /mnt/linux-root/swapfile-moss`；
2. 卸载 `/mnt/linux-root`，使 sda1 不再被 6.12 使用；
3. 停止并等待 watchdog pinger；
4. `sync`；
5. `mount -o remount,ro /`，必须确认 `root_ro_rc=0`；
6. 才执行 SysRq reboot；
7. 若 remount 失败，取消主动复位，保持现场并让硬件 watchdog 兜底。

修复后的 smoke 已输出 `root_ro_rc=0`，复位后的 sda2 `e2fsck -fn` 返回 0：

- [安全退出 smoke](../../hardware-linux-ls2k1000la-20260811-linux612-root-ro-cleanup-smoke.log)
- [退出后 fsck](../../hardware-linux-ls2k1000la-20260811-sda2-root-ro-cleanup-verify.log)
- [旧 orphan 修复记录](../../hardware-linux-ls2k1000la-20260811-sda2-post-buildstorm-repair.log)

## 8. 保留的镜像与恢复边界

当前主要文件为：

| 文件 | SHA256 / 状态 |
|---|---|
| `/srv/tftp/linux-6.12.0-lsgd-moss-lsx-uImage` | `e055584e04b0849d412f03028ab24bfd4a678529afc18399e04ae74618c88699` |
| `/srv/tftp/linux-6.12.0-lsgd-moss-uImage` | `9acb902acf24f78dd93dc87f41a6c2e5fbddd94e746f0ea758d151f5cea08183` |
| `/srv/tftp/moss-linux-rescue-uImage` | `f49c36145a80b4e7fdb7d0824b983a58a3118f2017c2e5618fb51973300dcf0d` |
| `/srv/tftp/sdcard-la-clean-12g-20260811.img` | 12,884,901,888 bytes；`35681b35f3fc73dbeec258a230e5781f22fd718b265c40f1ddfa668984be322d` |
| sda1 `/boot/uImage` | 原厂 Linux 5.10，保留 |
| sda1 `/boot/uImage.moss-linux-backup` | 原厂 Linux 5.10 磁盘备份，保留 |

所有旧内核继续保留。错误的 2 GiB BSP2 试验镜像只被隔离并标记为
`INVALID-DO-NOT-BOOT`，也没有删除。

## 9. 对 MOSS 优化的直接约束

Linux 6.12 的结果给 MOSS 提供了三个不能绕开的事实：

1. MOSS 必须把可分配物理内存限制为真实 1 GiB，不能继续继承原厂 DT 的虚假 2 GiB；
2. 1 GiB 下的完整多任务 Rust 冷编译存在真实匿名内存压力，不能用 1 GiB tmpfs 复制工具链和项目；
3. 在实现 MOSS swap 前，应优先使用可回收 page cache、MAP_PRIVATE mmap readahead/fault-around 和
   受控并发降低峰值；若仍出现可复现的 `NoMemory`，再以该证据实现匿名页换出，而不是假装增加 RAM。

## 10. Linux 结果如何落实到 MOSS

上述第 3 项的证据门槛已经满足。MOSS 在修正为 935 MiB 可管理物理页、启用 clean-only cache
reclaim、限制 heap expansion 并回收空 TLSF pool 后，仍在正式 buildstorm 的 1119/1178 秒附近以
`SIGBUS/OutOfMemory` 结束；失败时匿名 `VirtMem` 已约 400–450 MiB。因而 MOSS 已开始直接复用
sda1 的同一份 `/swapfile-moss`，而不是改变分区方案。

两套系统的使用边界不同：Linux 6.12 通过 ext4 挂载 sda1 后执行标准 `swapon`；MOSS 则在切换前
由 factory Linux 解析 swapfile 物理 extent，把校验后的绝对 LBA 通过 DT 传入，运行期只对这些块
执行 raw AHCI page I/O。两者都保留 swap header 页，且不会同时运行。sda2 继续只作为 12 GiB 比赛
根，原厂 5.10 和全部旧内核仍留在 sda1。

MOSS 的超时策略也与 Linux 6.12 runner 的“无限 workload timeout”目的保持一致：不存在固定的
20 分钟总运行上限。20 分钟只表示连续无用户任务推进的租约；用户任务集合或累计调度次数变化就
重新顺延 20 分钟。硬件 watchdog 本身仍是 30 秒，用于内核/调度器真正停止工作后的自动复位。

具体代码、换出并发安全边界和真机日志索引见主状态报告 20.21 节。当前修正版已通过 LS2K1000LA
两核冷构建和静态检查，完整 MOSS buildstorm 的最终 PASS/FAIL 以随后真机日志为准，不能用 Linux
6.12 的 PASS 代替。

## 11. MOSS 第一代 swap 结果与 Linux 的差异

MOSS 第一代 raw swap 已经越过旧的 1119/1178 秒 OOM 点，但未完成 buildstorm。1500 秒时仍有
200 MiB free、只使用 433 MiB/2047 MiB swap，说明失败不是交换空间不足；问题是累计换出
599,635 页、换入 487,494 页，rustc 在 1200–1500 秒间约 75% CPU 时间消耗在内核。1524 秒后
高频 I/O 最终使 AHCI command admission 超时，swap-in 返回 `Io`，rustc 收到 SIGBUS。

Linux 6.12 在同一工作负载中 swap 峰值仅约 197 MiB，完整构建用时 2663.92 秒；这个差异说明
MOSS 不能只实现“匿名页能写入磁盘”，还必须有工作集识别和抗抖动策略。第二代 MOSS 因此改成：

- 新分配、COW 和 swap-in 匿名页进入 120 秒软件活跃代，普通回收只选冷页；
- 真正的物理页分配失败可以越过活跃保护，避免假 OOM；
- 地址空间多轮回收到目标，单轮上限 8192 页；
- 双核用 wait queue 协调正在进行的回收，不能因另一核尚未归还页而直接失败。

工作集版在真机直到 507 秒才首次换出，证明抗抖动生效；但它随后暴露了 MOSS simple-ahci 的
非 NCQ admission 饥饿：交换线程连续重新抢锁，并发 ext4 读等待固定 1 秒后被误报为 I/O 失败。
第三代已把该通道改成 ticket FIFO，正常排队不再当作命令超时。

初版 FIFO 镜像两次都在 MOSS 运行 7.55 秒、根文件系统尚未访问时固定卡在 AHCI initial recovery，
并非交换 I/O 造成。ticket 字段改变结构布局后暴露了 LoongArch 大聚合对象搬移导致运行态原子字段
损坏的问题。因此最终实现让 `AhciPort` 从构造起就驻留在 `Box` 的稳定地址，并在地址确定后、发布
前显式重建 completion、FIFO ticket、锁、recovery 和 wait-mode 状态。当前真机镜像是
`/srv/tftp/moss-1g-working-set-stable-fifo-swap-wdt30-smp2.bin`，SHA256 为
`369da2626e3872d3c3ac24f442cefa316b10449243fd589b10845f56337f7822`，已通过 2 核/1 GiB 冷构建、
17 项 AHCI completion/recovery 测试、4 项 HAL wait-mode 测试和完整静态检查；v5 真机已越过前两轮
固定的 AHCI 初始化失败并进入 cagent，正在继续完整 buildstorm。完整 MOSS 因果链和逐轮日志见主
状态报告 20.21–20.22 节；最终仍以 MOSS 自身出现 `ok=true` 为验收，不能借用本报告中的 Linux PASS。

## 12. MOSS v5/v6：从“不崩溃”继续收敛到硬件反馈的工作集

稳定地址+FIFO 的 v5 已越过 v3 的 521 秒 AHCI admission 崩溃点，1200 秒仍无 I/O 错误；其累计
换出/换入 180,224/116,689 页，低于第一代同刻的 256,482/191,514，但固定 120 秒保护仍会让已
refault 的页面周期性再次成为候选。1200 秒 rustc 的 user/system ticks 为 36,340/48,007，说明
编译持续推进，但交换成本仍明显高于 Linux 6.12。

下一版不再继续改时间常数，而是利用 LA264 PTE.D 实现写访问二次机会：清 D 保留 W，真实写入
通过 PageModify fault 晋升页面；观察期内未写的页才换出。已经 swap-in 的 refault 页退出主动
回收，只有真实分配失败才能强制越过保护。与终端信号重构合并后的最终 v6 镜像
`/srv/tftp/moss-1g-dirty-second-chance-tty-signal-wdt30-smp2-v6.bin` 大小为 3,908,224 字节，SHA256
为 `5b47716688a51ada74d2dac2cc183be679d586a57f4602653ebd3db155ab647c`，已通过两核/1 GiB 冷构建和
完整静态检查。v6 同时按每核 WFI 空闲时间输出 5 分钟区间利用率，避免把阻塞 syscall 的 `stime`
误当成计算负载；滑动租约只由任务身份变化或达到单核 10% 的真实非空闲 CPU 工作续期，不再被
cargo 周期性 futex 空唤醒欺骗；活动 syscall 同时输出前三个参数，可直接定位阻塞写的 fd 和长度。

终端信号不再依赖轮询或 buildstorm 特判：tty reader 就绪后才启用 NS16550 RX IRQ，IRQ source
按 one-shot 屏蔽/重臂，n_tty 依据 `ISIG` 向真实前台进程组发送 SIGINT/SIGQUIT/SIGTSTP，
`TIOCSPGRP` 也改为读取并校验用户 pgid。真机 smoke 中 cagent 和 minibuild 完成后，在 buildstorm
前台运行时发送 `0x03`，42.993 秒立即出现 `MOSS_TTY_SIGNAL signo=SIGINT foreground_pgid=10`，
随后受控重启回原厂 Linux，没有等 20 分钟租约或硬件看门狗。最终 v6 已重新开始不注入 Ctrl+C 的
完整 buildstorm 长跑。永久 Linux 6.12 镜像、原厂 5.10 回退和 sda1 swapfile 均保持不变。

## 13. MOSS v8：后台回收与真正的用户 CPU 进展租约

v6 真机完整跑证明 PTE.D 二次机会的正确性成立，但同步触发方式不可用：1500 秒时仍停留在第一项
构建的 23 条 `Compiling`，1200--1500 秒 rustc user ticks 只增加 344，system ticks 增加 29,716；
每次 1--3 页的小批回收与逐批串口 warn 消耗了 CPU0 约 98.7% 的区间时间。v8 因而把低/高水位
维护移到单一后台 worker：192 MiB 触发、每批 2048 页、补到 256 MiB，PTE.D 观察窗为 5 秒；只有
低于 64 MiB或真实分配失败才同步强制回收。过程日志汇总进 5 分钟遥测。

为防后台 worker 自己把停滞租约续上，`axtask` 由 timer tick 单独累计 userspace-owned task 的真实
CPU 纳秒。v8 初版仍把后台实际释放页计为显式进展；复核后确认持续 swap 抖动也可能借此无限续租，
因此严格版 v9 已删除该续租点，后台空扫和成功换出都不算用户进展。进程信号同时
由“只唤醒第一个候选 tid”改成“pending 一次、唤醒所有未屏蔽 tid”，修复多线程退出竞态下的
Ctrl+C 丢唤醒。最终镜像
`/srv/tftp/moss-1g-background-reclaim-signal-wakeup-wdt30-smp2-v8.bin` 大小 3,916,416 字节，
SHA256 `a3891be91b38f1fd12addfa51cb949af73b88e48f150b74b4fc1085442362f31`，全部静态检查通过。
旧 v6 已由串口 `Ctrl+\\` 触发 SIGQUIT 后受控 reboot，不再需要物理 reset；runner 随后装载 v8。
v8 到 900 秒一直只有后台回收，1200/1500 秒因 free 越过 64 MiB 紧急线而累计同步回收
65,536/147,456 页；尽管其 rustc 用户态进度显著好于 v6，1519 秒仍因 `ECODE=0、ESTAT.IS=0`
的 level-triggered 空中断被旧 trap 分派误判 `Unknown` 而 panic，随后自动 reboot 回 Linux。

当前 v10 同时合入严格租约、spurious IRQ 精确判定以及高阶 folio 失败退避：64 KiB 连续页分配
失败后 30 秒内直接使用小 folio，每分钟最多输出一条累计汇总，并在 5 分钟遥测公开失败次数。
镜像为 `/srv/tftp/moss-1g-background-reclaim-signal-spuriousirq-folio-backoff-wdt30-smp2-v10.bin`，
SHA256 `c1e05c8c8781b84ce7dc8db600104069448d7b483edcf3772913a88d8de8df1b`，完整 LS2K 静态和
交叉构建检查通过，现已自动装载并重新进入 buildstorm。永久 Linux 6.12、原厂 5.10、sda1
swapfile 和 sda2 比赛根均未改动。
