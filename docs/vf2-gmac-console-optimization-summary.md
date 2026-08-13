# VF2 GMAC1 与双板串口交互优化总结

本文汇总 2026-08-13 完成的两项改进：

1. 在 VisionFive 2 v1.3B 上完善 JH7110 GMAC1 实体网卡驱动；
2. 降低 VisionFive 2 和 LS2K1000LA 进入 MOSS shell 后的串口输入回显延迟。

对应实现提交分别为 `d803c88e`（`feat: 完善 VF2 GMAC1 以太网驱动`）和
`4813447b`（`fix: 降低实板串口输入回显延迟`）。本文重点说明修改前有什么问题、问题为什么发生、
实现如何消除这些问题，以及当前结论能够覆盖到什么程度。具体上板命令和逐项验收方法见：

- [VisionFive 2 GMAC1 驱动实现与验收](visionfive2-gmac1驱动实现与验收.md)；
- [VF2 与 LS2K1000LA 串口输入延迟修复和验收](board-console-input-latency.md)。

## 1. 结论概览

| 优化 | 修改前的核心问题 | 修改后的核心路径 | 当前验收状态 |
| --- | --- | --- | --- |
| VF2 GMAC1 | 平台资源和链路模式过度依赖硬编码/U-Boot；固定 1000/full；DMA、IRQ 和异常恢复生命周期不完整 | DT 驱动的 probe、first-use DMA、Clause 22 链路状态机、非一致性 DMA、deferred IRQ 和错误恢复 | 静态状态测试、两套 DT、VF2/LA/RV 构建通过；本轮未连接 VF2 复测断链、三速和 APK |
| 双板串口 | RX 唤醒不触发及时调度；同步输出整段持有关 IRQ UART 锁；N_TTY 逐字符回显 | TTY 专用低延迟 Waker、字符粒度 UART 锁、可重入记录锁、批量回显 | VF2/LA 构建及 RV/LA QEMU 压力回归通过；本轮未连接实板测量延迟分位数 |

两项优化解决的是不同层次的问题。VF2 GMAC1 使用 JH7110 板载 DWMAC 5.20/EQoS 和 YT8531 PHY，
不是 `virtio-net`；QEMU 中的 RV/LA 网络仍走 `virtio-net`。串口改动位于两块实板共用的 TTY、调度、
日志和同步 UART 输出路径，不属于网卡驱动。

## 2. VF2 GMAC1：原实现存在的问题

修改前已经有一个可以完成基础收发的 GMAC1 驱动雏形，但它更接近“沿用 U-Boot 已配置状态”的
最小实现，还没有形成可长期运行的完整驱动闭环。

### 2.1 设备发现和配置依赖硬编码

旧 probe 只从 `/aliases/ethernet1` 读取 MAC 地址，MAC 基址、IRQ、PHY 地址、RGMII syscon 偏移、
时钟和 reset 都来自固定值。它没有验证目标节点是否启用、compatible、寄存器窗口、`phy-mode`、
`phy-handle` 和 DMA 参数，也没有区分主线 Linux DT 与 StarFive U-Boot 2021.10 DT。

这会带来两个问题：

- 固件 DT 版本变化时，代码可能继续访问错误资源，而不是在 probe 阶段明确拒绝；
- VF2 v1.2A 的 RMII 和 v1.3B 的 GMAC1 RGMII-ID 接线可能被混为一谈。

### 2.2 MAC 链路模式固定为 1000/full

旧驱动直接把 MAC 配成 1000 Mbps、full duplex，并假设 U-Boot 已完成 PHY 协商。运行后没有读取
ANAR/ANLPAR 和 1000BASE-T 状态，也没有在拔线、重新插线或交换机降到 100/10 Mbps 后重配 MAC、
flow control 和 YT8531 TX clock phase。

因此基础千兆连接即使能够工作，也不能推出以下路径可靠：

- 10/100/1000 Mbps 自动协商；
- link down 后停止收发，link up 后恢复；
- 暂时性 MDIO 失败与真实断链的区分。

### 2.3 DMA 生命周期、ring 和缓存一致性缺少完整约束

旧 `try_new()` 在 probe 阶段立即 reset DMA、初始化 ring 并启动收发。此时网络栈可能尚未接管设备，
DMA 已经能够写入内存。描述符步长、tail pointer 语义和 DMA 参数也以固定寄存器值表达，没有把
Linux DT binding、硬件 FIFO 能力和 JH7110 的 8 字节 AXI data width 纳入可测试策略。

JH7110 GMAC 是 non-coherent DMA，CPU 与 DMA 每次移交 descriptor/buffer 所有权都必须维护 cache。
此外，旧 RX 路径在网络包池分配失败时会提前返回，当前 descriptor 来不及重新交还 DMA，也不会推进
consumer index；这会减少 DMA 可用的 RX 空间，并可能在 ring 压力下停止推进。

### 2.4 中断和错误处理不足

旧硬中断能够确认 DMA status 并记录事件，但 deferred poll 只回收 TX，并在 fatal bus error 时打印
warning。它没有闭环处理 `RBU`、`RPS`、`TBU`、TX descriptor error、`TPS` 和 `FBE`，也没有在
“检查队列—重新开中断”的窗口再次检查状态。

结果是异常发生后可能出现 ring 停止推进、丢失唤醒、重复中断或只能重启系统恢复的问题。

### 2.5 接收过滤过宽

旧实现设置 `PR`，长期处于 promiscuous mode；同时没有完整表达 Address0 enable、广播、组播回退和
FCS/pad 剥离策略。这样既接收了不属于本机的单播帧，也可能把硬件报告长度中的 FCS 交给网络栈。

## 3. VF2 GMAC1：优化是怎么做的

### 3.1 用 DT 建立明确的硬件契约

平台层 [jh7110.rs](../vendor/axdriver/src/jh7110.rs) 现在从 `/aliases/ethernet1` 定位 GMAC1，并验证：

- 节点启用状态、compatible、register window 和 IRQ；
- `phy-mode = "rgmii-id"`；
- 主线 DT 的 `phy-handle`，或旧 U-Boot DT 中限定在当前 MDIO 子树内的 legacy PHY；
- DMA PBL、AXI burst、FIFO 和 syscon 参数。

驱动接受 `starfive,jh7110-dwmac` 和 `starfive,jh7110-eqos-5.20` 两种格式，只支持 VF2 v1.3B
的 GMAC1 RGMII-ID。时钟、AXI/AHB reset 和 RGMII syscon 在 probe 中显式配置，不再把正确性完全
交给 U-Boot 的遗留状态。

### 3.2 把 probe 和 DMA 启动分开

[dwmac_jh7110.rs](../vendor/axdriver_net/src/dwmac_jh7110.rs) 的 probe 只取得单实例 DMA storage、停止
旧 DMA、读取 MAC/PHY 状态并注册设备。第一次 `receive`、`transmit` 或 TX reclaim 才进入
`ensure_started()`，完成：

```text
stop/reset DMA
  -> 初始化 RX/TX ring
  -> 配置 MTL、MAC、AXI 和 DMA
  -> 启动 DMA engine
  -> 读取 PHY 并按实际链路模式启用 MAC
```

这样 DMA 只有在网络栈真正使用设备后才获得 buffer 所有权。`DmaStorage::take()` 还保证静态 ring
不会被第二个设备重复占用。

### 3.3 明确 descriptor、tail 和 cache ownership

TX/RX ring 各有 64 个 descriptor，每项扩展到一个 64 字节 cache line。DWMAC 基础 descriptor
为 16 字节，JH7110 AXI data width 为 8 字节，因此 DSL 按 `(64 - 16) / 8 = 6` 编码，不再使用
不透明的固定常量。

所有权转换按固定顺序执行：

```text
buffer/descriptor clean 或 invalidate
  -> fence iorw, iorw
  -> 写 OWN 或 tail pointer
```

RX tail 使用 inclusive boundary，TX tail 使用 exclusive producer boundary。非法 RX 帧或 packet
buffer 分配失败也必须重新 arm descriptor，避免一次 drop 永久缩小 ring。

### 3.4 增加运行期 PHY 链路状态机

驱动每秒轮询 Clause 22 状态。BMSR 的 link 位具有 latch-low 语义，因此连续读两次；自动协商完成后
解析 10/100/1000 Mbps、duplex 和 pause，再同步更新：

- MAC `PS`、`FES`、`DM`；
- TX/RX flow control；
- YT8531 对应速率的 TX clock inversion；
- MAC TX/RX enable 状态。

明确读到 link down 才关闭 MAC；暂时性 MDIO 失败保留上一次有效状态，避免把总线瞬态错误误判为拔线。

### 3.5 中断只搬运事件，恢复在 poll 路径完成

硬中断读取、屏蔽并 acknowledge 已触发 source，再通过 atomic bitmap 把工作交给 `poll_complete()`。
deferred poll 负责 RX/TX、TX reclaim、PHY refresh 和错误恢复；排空后重新开中断，并再次读取 status
覆盖竞争窗口。

恢复策略为：

| 事件 | 处理 |
| --- | --- |
| `RBU` / `RPS` | 重新发布最后归还的 RX descriptor |
| `TBU` | 重新发布 TX producer boundary |
| TX descriptor error | 记录 index/status 后继续回收 |
| `TPS` / `FBE` | 停止并 reset DMA、重建 ring、重新读取 PHY 后启动 |

IRQ 注册失败时仍保留 polling fallback，不把基本网络功能绑定到中断必须成功。

### 3.6 收紧接收过滤并与 LA 共享状态规则

驱动启用 Address0 perfect unicast 和广播，清除固件遗留的 `PR`/`RA`。由于当前网络接口没有动态
multicast-list 回调，使用 all-multicast 作为明确回退，而不是保持全混杂。`ACS` 和 `CST` 同时启用，
保证 pad/FCS 不交给上层。

VF2 EQoS 5.20 与 LS2K1000LA DWMAC 3.x 的寄存器布局不同，不能共用具体硬件代码；两者只共享
[dwmac_common_state.rs](../vendor/axdriver_net/src/dwmac_common_state.rs) 中可验证的 Clause 22、链路转换、
DMA lifecycle、RX rearm 和 TX ring ownership 规则。这样实现了“向 LA 看齐”，同时没有把两套硬件
差异强行合并。

## 4. 双板串口：原实现存在的问题

问题现象是 U-Boot 中输入正常，进入 MOSS shell 后单键回显变慢，持续日志输出时更明显。VF2 和
LS2K1000LA 使用不同 UART 平台实现却出现相同现象，说明主要等待位于共享的 TTY、调度和日志路径。

### 4.1 RX 唤醒只入队，不请求抢占

UART RX IRQ 把输入放入 8 KiB 软件 ring 后唤醒 `tty-reader`。旧 `AxWaker` 使用
`unblock_task(task, false)`，只把任务放回 run queue：

- 同核有任务运行时，要等对方主动让出 CPU 或调度 tick；
- 跨核目标 CPU 忙时，只对 idle CPU 发送 kick，不会请求忙核重新调度。

当前 tick 为 100 Hz，即每 tick 10 ms；RR 时间片为 5 tick。忙核上的交互任务因此可能等待一个
完整时间片量级，约 50 ms，之后才开始处理已经到达的软件 ring。

### 4.2 同步 UART 输出整段关闭本地 IRQ

两块板的 `write_bytes()` 都在整个 buffer 输出期间持有 `SpinNoIrq` UART 锁。115200 8N1 每个字符
约需 `10 / 115200 = 86.8 us`，输出 1 KiB 至少占用约 88.9 ms。该锁持有期间本地 IRQ 关闭，所以
同核 UART RX 即使已经到达也不能被搬入软件 ring。

这解释了为什么持续日志或命令输出会进一步放大输入延迟。

### 4.3 N_TTY 逐字符回显和日志关 IRQ 继续放大等待

旧 N_TTY 对每个输入字符调用一次 `writer.write()`；退格等字符还会展开成多个字节。每次调用都重新
经过锁和 MMIO 路径。日志层则在格式化和发送整条记录期间持有 IRQ-safe 锁，长记录会再次形成与
整段 UART 写相同的 RX 阻塞窗口。

## 5. 双板串口：优化是怎么做的

### 5.1 只为 TTY RX 增加低延迟唤醒策略

[ldisc.rs](../kernel/src/pseudofs/dev/tty/terminal/ldisc.rs) 的外部输入 worker 改用
`block_on_latency_sensitive()`。该 Waker 唤醒时传入 `resched = true`：

- 同核唤醒设置当前任务的 preempt pending；
- 跨核唤醒通过合并的 reschedule IPI 请求目标 CPU 调度；
- pending 与 in-flight 状态在真正选择下一任务或收到 IPI 后清除，避免重复 IPI。

普通 Future 继续使用原 `block_on()`，不会因为串口交互需求给全系统所有异步唤醒都增加抢占和 IPI。

### 5.2 把硬件锁缩短到一个物理字符

VF2 和 LS2K1000LA 的板级 `write_bytes()` 都改为每发送一个字符获取一次 UART 锁。上层负责保持
普通 write 或日志记录的整体顺序，底层只保证单个 MMIO 字符发送安全。

因此 RX IRQ 最长只被一个字符的硬件临界区阻塞，而不是被完整输出 buffer 阻塞。对 `\n` 展开的
`\r\n` 也是两个独立字符边界。

### 5.3 用记录锁保持输出顺序，同时允许 RX IRQ

[axlog](../vendor/axlog/src/lib.rs) 增加共享 console record lock：

- 完整普通 TTY write 和日志记录仍保持原子顺序；
- 持锁期间只禁止任务抢占，不长期关闭 IRQ；
- owner 通过 CPU/task identity 记录，IRQ 使用被中断任务身份重入，避免中断日志等待自己；
- 其他 CPU 只在更新短小的 ownership metadata 时关 IRQ。

这样同时满足“日志不能互相穿插”和“RX IRQ 能在长记录的字符边界运行”。终端自动回显使用单独的
`write_echo()`，允许在普通长输出的字符边界插入；PTY 沿用普通 write 顺序。

### 5.4 一次批量提交本轮输入回显

N_TTY 每轮最多读取 80 字节，单字符最坏会扩展为 3 字节退格序列，因此使用 240 字节栈上
`EchoBuffer`。换行、回车、退格、`ECHOCTL` 和 Ctrl-C 的转换先写入该 buffer，处理完本轮输入后
只调用一次 `write_echo()`。

这减少了逐字符函数调用和锁竞争，同时保留原有终端语义。单元测试覆盖了普通字符、控制字符和
80 字节最坏扩展，不依靠删减回显功能换取低延迟。

## 6. 验证结果与边界

### 6.1 VF2 GMAC1 静态和构建验证

实现提交前已执行：

```bash
scripts/check_vf2_gmac_static.sh
scripts/check_ls2k1000_static.sh
make kernel-rv
make kernel-la
```

结果包括 6/6 共享状态测试、6/6 JH7110 策略测试、主线/旧 U-Boot 两套 DT fixture、VF2 DMA
section 和 VF2/LS2K1000LA/RV/LA 构建全部通过。此前实板基础双向 TCP 通过，但本轮没有 VF2 实板，
因此断链恢复、三速协商和公网 `apk update`/`apk fetch` 仍是待上板项目。

### 6.2 串口构建和 QEMU 回归

串口改动完成后已通过 VF2、LS2K1000LA 和两套 QEMU release 镜像构建。2026-08-13 又分别在 RV
和 LA QEMU 中连续启动三次，每次按 `src/init_final.sh` 依次运行 cagent 和 BuildStorm：

| 架构 | cagent | BuildStorm | BuildStorm 三轮耗时 | QEMU 退出 |
| --- | --- | --- | --- | --- |
| RV | 每轮 10/10，3/3 轮通过 | 3/3，`ok=true` | 484.64 / 499.97 / 492.70 s | 三轮均为 0 |
| LA | 每轮 10/10，3/3 轮通过 | 3/3，`ok=true` | 372.63 / 381.54 / 371.93 s | 三轮均为 0 |

六份日志没有发现 kernel panic、Page Fault、BUG、timeout 或 QEMU error。该回归证明两套架构的
调度、TTY/日志相关代码在 cagent 和高并发编译压力下没有出现已知功能回退，但 QEMU 使用不同的
虚拟串口，不能代替 VF2/LS2K1000LA 的 115200 波特率实板延迟测试。

### 6.3 实板完成标准

当前不能写成“串口 P95 已达标”或“VF2 APK 已拉取成功”，因为本轮明确没有连接目标板。完整完成
仍需执行：

```bash
python3 scripts/test_board_console_latency.py --serial /dev/ttyACM0
sudo scripts/run_vf2_gmac_test.py --speed-suite --apk-package busybox
```

串口脚本要求空闲 P95 不超过 20 ms、持续输出 P95 不超过 30 ms，并校验 22/262/518 字节粘贴的
SHA-256。GMAC 脚本要求基础链路、双向 TCP、link flap、三速、DNS、HTTPS、`apk update`、
`apk fetch` 和实际 `*.apk` 文件逐层通过。只有保留对应原始日志后，才能更新为完整实板验收通过。
