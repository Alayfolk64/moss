# VisionFive 2 移植与调试工作总结

本文总结截至 2026-08-13，Moss 在 StarFive VisionFive 2（JH7110）上的移植、驱动接入、问题定位和实板验证结果。日常构建、写卡、TFTP 和 U-Boot 命令见 [VisionFive 2 上板常用指令](visionfive2板-启动指令.md)。

## 1. 当前结论

Moss 已能在 VisionFive 2 上稳定完成以下流程：

- 由 OpenSBI 和 U-Boot 进入 S-mode，在 `0x40200000` 启动 Moss；
- 识别 4 GiB 内存和 4 个 U74 S-mode hart；
- 通过 PLIC 分发外部中断，并支持时钟中断和核间中断；
- 通过 UART 中断进行串口输入，进入可交互 shell；
- 从 SD 卡挂载整盘 ext4 rootfs，并完成持久化读写；
- 使用板载 GMAC1 与 PC 双向进行 TCP/IP 通信；
- 运行 glibc 用户程序、cagent、Git 基础操作和板载 `rustc`。

目前 VF2 已具备 Moss 基础功能验证和用户态工作负载调试条件，但 SD 卡仍使用 PIO 搬运数据，eMMC、USB、显示等外设尚未接入。

## 2. 前期确认与启动介质

实板固件环境为 OpenSBI v1.2 和 StarFive U-Boot 2021.10。OpenSBI 使用物理 hart 1 作为启动 hart，hart 0 不提供普通 S-mode 运行环境。

对 StarryOS/TGOSKits 中 VF2 基线的检查和实测表明：已有实现可作为 JH7110 启动、串口和 `simple-sdmmc` 的参考，但不能直接视为 Moss 已完整支持 VF2。基线启动日志中 SD 卡可以识别，而网卡数量仍为 0；Moss 后续对平台层、中断、网络、SMP 和文件系统路径都进行了补充。

U-Boot 中两路 MMC 的对应关系是：

| U-Boot 设备 | 控制器地址 | 当前用途 |
| --- | --- | --- |
| `mmc 0` | `sdio0@16010000` | eMMC，当前 Moss 未接入 |
| `mmc 1` | `sdio1@16020000` | SD 卡，当前 rootfs 设备 |

当前使用方式为：

- Moss 内核由 U-Boot 通过 TFTP 加载到内存；
- SD 卡只保存 rootfs，不保存正在运行的 Moss 内核；
- rootfs 镜像是整盘 ext4，应写入 `/dev/sdX`，不能写入 `/dev/sdX1`。

早期曾把 ext4 镜像写进 GPT 分区，导致 Moss 将整张卡当作 ext4 时挂载失败。清除分区表并把镜像直接写到整卡后解决。

TFTP 使用的是 U-Boot 自带网卡驱动，因此“TFTP 能传内核”只能证明 U-Boot 网络正常，不能证明 Moss 网卡驱动已工作。

## 3. 平台启动支持

新增了独立的 `axplat-riscv64-visionfive2` 平台实现，并提供：

- `make kernel-vf2` 构建入口，产物为 `moss-vf2.bin`；
- VF2 的内存、MMIO、PLIC、UART、SD/MMC 和 GMAC1 配置；
- U-Boot/OpenSBI 直跳入口和早期 Sv39 页表；
- 启动阶段 DTB 保留与解析；
- SBI timer、HSM、IPI 和 system reset 接口；
- 主核和从核的独立启动入口。

启动时保留 U-Boot 提供的 DTB，并将固件、DTB、内核段和空闲内存正确排除或加入内存布局。实板启动结果能够识别 4 GiB RAM，不再把固件或 DTB 所在区域交给页分配器。

## 4. 中断体系

VF2 当前使用三类 RISC-V S-mode 中断：

| 类型 | 入口 | 用途 |
| --- | --- | --- |
| Supervisor timer interrupt | STIP | 调度时钟和定时器 |
| Supervisor software interrupt | SSIP | SMP 核间通知和 TLB shootdown |
| Supervisor external interrupt | SEIP | 进入 PLIC，分发 UART、SD/MMC、GMAC 等设备中断 |

JH7110 的 hart 0 没有对应的 S-mode PLIC context。Moss 的逻辑 CPU 0..3 分别对应物理 hart 1..4，PLIC context 也按该映射计算，不能直接把逻辑 CPU ID 当成物理 hart ID。

PLIC 初始化会清除固件遗留的 source enable，驱动注册处理函数后再启用对应中断源。未注册的外部中断源会被屏蔽，避免形成中断风暴。

## 5. UART 串口

JH7110 UART 是 8250 兼容设备，但寄存器步长为 4 字节。Moss 沿用 U-Boot 已配置的时钟和波特率，不在接管阶段重新初始化硬件。

完成的改动包括：

- UART RX 接入 PLIC，不再由 shell 持续轮询硬件；
- 使用与 LS2K1000LA 共用的无堆分配 8 KiB 软件接收环；
- UART 中断每轮最多抽取 256 字节并批量排空硬件 FIFO，释放锁后再唤醒 TTY；
- PLIC handler 注册成功后才开启 UART RX interrupt，并保持设备中断持续启用，不再使用会在
  每次唤醒后屏蔽 source 的通用 one-shot IRQ waker；
- SMP 输出使用同一把锁保持单次写入连续；
- 修正终端行规程中的退格、控制字符和信号处理。

早期 shell 中出现的按键无响应、退格显示 `^H`、方向键输出转义字符、命令残留后进入 `>` 提示等问题，属于 UART 输入与 TTY 行规程缺陷，不是 VF2 对 shell 命令格式的特殊要求。

2026-08-12 的这次 RX 重构已通过 `make kernel-vf2`。本轮 VF2 未连接，因此这里不把构建通过
写成新的实板长粘贴验收；下次上板应保持回显开启，用一次串口写入复测 22、262 和 518 字节
完整命令。

## 6. SD 卡与 ext4

当前 Moss 只探测 `0x16020000` 的 SD 控制器，即 U-Boot 中的 `mmc 1`。驱动以已有 `simple-sdmmc` 为基础接入 Moss 块设备框架，并补充了：

- SD/MMC PLIC 中断注册和等待队列；
- 命令超时、数据超时和控制器恢复；
- 读写错误向块设备和文件系统返回，替代 `unwrap()` 整机 panic；
- 块号范围检查和有限次数重试；
- `flush()` 等待卡完成编程，保证 `sync/fsync` 语义；
- 从 HCON 读取 FIFO 数据宽度；
- 将 FIFO 作为固定 MMIO 数据端口访问，而不是连续地址窗口。

实板 HCON 表明 VF2 的 host data width 为 32 位。原驱动使用 64 位访问并随数据偏移递增 MMIO 地址，这与 DesignWare MMC FIFO 的工作方式不符，现已改为在同一 FIFO 地址进行 32 位读写。

SD 数据阶段目前仍是 PIO。为了避免任务睡眠后不能及时处理 RXDR/TXDR 而触发 host timeout，数据搬运阶段会保持当前 CPU 活跃并轮询控制器 raw status；PLIC 仍用于控制器中断接管和其他命令路径。因此 SD 卡尚未达到“全程中断且 DMA 搬运”的状态。

### ext4 追加内容变成零的问题

实测曾出现以下现象：文件首次写入正确，追加后在页缓存中也正确，但执行 `sync` 并丢弃缓存后，追加区域变成等长的零字节。

逐层探针确认：

1. 页缓存交给 ext4 的数据正确；
2. ext4 计算出的物理块和交给块驱动的数据正确；
3. 真正覆盖数据的是随后提交 inode 长度的路径。

根因是页缓存先写回完整数据，之后又调用普通 `set_len(old_len -> new_len)`。普通扩容按 POSIX 语义会把旧 EOF 到块尾清零，因此刚写入的追加内容被二次覆盖。

修复后区分两种操作：

- 用户主动执行 `truncate/ftruncate` 扩容：新增区域继续补零；
- 页缓存写回后提交新长度：只更新 inode size，不再次清零数据。

验证结果：

- 写入 `FIRST_BLOCK` 后 `sync`、丢弃缓存并实读，内容正确；
- 追加 `SECOND_BLOCK` 后重复上述操作，两段内容均正确；
- 再用 `truncate` 扩容到 32 字节，新增的 7 字节仍全部为零。

## 7. GMAC1 网络

本节记录结果概要；完整驱动结构、硬件契约、错误恢复和 APK 分层验收见 [VisionFive 2 GMAC1 驱动实现与验收](visionfive2-gmac1驱动实现与验收.md)。

为 JH7110 GMAC1 接入了 DesignWare MAC 驱动，完成：

- GMAC1 时钟启用和 AXI/AHB reset 解除；
- 仅接受 VF2 v1.3B 的 `rgmii-id` 接线，明确拒绝 v1.2A RMII；
- 同时解析主线 Linux `starfive,jh7110-dwmac` 和 StarFive U-Boot 2021.10 `starfive,jh7110-eqos-5.20` DT；
- 从 DTB 的 `ethernet1` 读取寄存器、IRQ、MAC、PHY、FIFO、PBL、AXI burst 和 syscon 配置；
- 描述符环、TX/RX 数据路径和缓存一致性处理；
- probe 阶段保持 DMA 停止，第一次网络队列操作才启动；
- 双读 Clause 22 BMSR，并随链路协商结果更新 10/100/1000 Mbps、双工和 pause 流控；
- 识别 YT8531 后按速率更新 TX 时钟反相位，PHY delay 和驱动强度仍沿用 U-Boot 的冷启动配置；
- 中断处理按 RX/TX 源屏蔽并在 deferred poll 后重开，注册失败时保留轮询降级路径；
- RX 丢包和内存不足路径总会归还描述符，fatal DMA bus error 会完整停止、reset 并重建队列；
- 与 Moss `axnet`/TCP/IP 栈对接。

实板已经完成 Moss 与直连 PC 之间的双向 TCP 数据收发。当前只接入 GMAC1，即 U-Boot 日志中的 `ethernet@16040000`。

2026-08-13 的链路管理与错误恢复增强已通过 host 状态机测试、两套 DT fixture 和 VF2 交叉构建；由于当前没有接入 VF2 实板，本轮新增的断链恢复、三速重协商及 `apk fetch` 自动化脚本尚未执行，不能沿用此前基础 TCP 结果写成“增强路径已实板通过”。实板命令见 `scripts/run_vf2_gmac_test.py`。

`git clone https://github.com/...` 未成功并不等同于网卡驱动失败。当时直接原因包括 rootfs 中 DNS 解析不可用，以及缺少 `ssh-keygen`/SSH 客户端；访问公网 Git 还需要 DNS、HTTPS CA 证书和相应用户态工具共同可用。

## 8. 四核 SMP

VF2 的 4 个 U74 核已经全部启用：

- 逻辑 CPU 0..3 映射到物理 hart 1..4；
- 从核通过 SBI HSM `hart_start` 启动；
- 每核建立启动栈、陷阱入口、PLIC context 和 timer；
- IPI 目标使用物理 hart ID；
- TLB shootdown 支持跨核通知。

实板启动标志为：

```text
MOSS_DTB cpu_nodes=4 memory_regions=1 memory_bytes=0x100000000
MOSS_SMP online=4
```

用户态 `cat /sys/devices/system/cpu/online` 和 `nproc` 应分别得到 `0-3` 和 `4`。

## 9. 用户态与系统调用问题

在 cagent 和现场赛 rootfs 验证过程中，还修复或确认了以下问题：

- ELF 加载失败不再因内部 `unwrap()` 导致整机 panic；
- 修正 wait/signal/进程退出清理，使 cagent 的并发子进程能够完整回收；
- 降低正式构建日志等级，避免每次进程进入用户态、退出和 `waitpid` 都刷屏；
- 修正 tmpfs/dentry 缓存一致性问题，避免 Git 已提交后重新查找仍看到旧目录状态；
- shell 模式改用 rootfs 中标准 `/bin/sh`，并保持其他构建目标的启动方式兼容；
- 官方现场赛 ext4 rootfs 能作为 VF2 rootfs 使用，不需要把 Moss 内核写进该镜像。

板载 `rustc helloworld.rs` 已成功生成并运行程序，输出 `Hello, VF2!`。该次小程序编译约耗时两分多钟，主要受 SD PIO 和板端工具链冷读影响，并非编译器卡死。

## 10. 已完成的实板验证

| 项目 | 结果 |
| --- | --- |
| U-Boot TFTP 加载 Moss | 通过 |
| 4 GiB 内存识别 | 通过 |
| 4 核 SMP | 通过 |
| PLIC、timer、IPI | 通过基础启动与调度验证 |
| UART 输出和交互输入 | 通过 |
| SD 卡识别和 ext4 挂载 | 通过 |
| SD 持久化写入、追加、清缓存重读 | 通过 |
| 显式 `truncate` 补零语义 | 通过 |
| `/proc/cpuinfo`、`/proc/meminfo` | 通过 |
| cagent glibc 测试组 | 全部结束并报告 pass |
| loopback TCP 服务 | 通过 |
| VF2 与 PC 双向 TCP | 通过 |
| GMAC1 断链恢复、1000/100/10 Mbps 重协商 | 已提供自动化脚本，本轮未接实板执行 |
| `apk update` / `apk fetch` 公网分层验证 | 已提供可选脚本，本轮未接实板执行 |
| Git 初始化、暂存和提交路径 | 已运行并修复相关缓存问题 |
| 板载 Rust Hello World 编译运行 | 通过 |

本轮最终修改还完成了以下构建回归：

```text
make kernel-vf2
make kernel-rv
make kernel-la
make kernel-la-2k1000
```

四个目标均构建成功，VF2 改动没有破坏 RISC-V QEMU、LoongArch QEMU 和 2K1000LA 的构建。

## 11. 仍未完成的主要工作

1. **SD/MMC DMA**：当前 PIO 正确但性能较低，`rustc`、大型构建和大量小文件 I/O 会明显受限。
2. **eMMC**：当前只支持 SDIO1/SD 卡，尚未为 `sdio0@16010000` 完成探测、初始化和双设备选择。
3. **公网用户态环境**：需要补齐 DNS 配置、CA 证书、SSH 客户端和 `ssh-keygen`，才能完整验证 HTTPS/SSH Git clone。
4. **存储压力测试**：还应执行长时间随机读写、反复掉缓存、重启后 `e2fsck` 和大规模源码构建。
5. **更多外设**：USB、PCIe/NVMe、HDMI/显示、音频、GPIO 等尚未适配。
6. **电源管理**：关机通过 SBI system reset 的 shutdown 类型进入安全停止；是否真正切断板卡电源取决于 OpenSBI、U-Boot 和 PMIC，电源灯保持点亮不代表文件系统没有完成关机。

## 12. 对应提交

本次 VF2 工作可按以下提交追踪：

| 提交 | 内容 |
| --- | --- |
| `639bb713` | 建立 VF2 平台并实现首次实板启动 |
| `8216e8a2` | 完成 shell/cagent 实板验证并修复 ELF、TTY、signal 等问题 |
| `f21bd66f` | 接入 VF2 PLIC 框架 |
| `415a23ac` | 恢复 PLIC 外部中断并接入 UART RX 中断 |
| `fc78c831` | 接入 SD 卡驱动 |
| `df6cb5e2` | 接入 JH7110 GMAC1 网卡驱动 |
| `508c29a3` | 启用 4 核 SMP 并兼容标准 `/bin/sh` rootfs |
| `925bab5b` | 修复 SD/ext4 写回数据清零并完善 FIFO 和错误恢复 |

## 13. 总结

这轮工作的结果不是简单地让 VF2 打印启动日志，而是打通了从固件启动、平台中断、串口交互、SD/ext4、物理网络、四核 SMP 到 glibc 用户程序的完整路径。Moss 已可在 VF2 上承担基础功能验证和用户态应用调试；下一阶段的重点应从“能运行”转向 SD DMA、eMMC、公网用户态环境和长期稳定性。
