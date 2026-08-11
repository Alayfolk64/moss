# LS2K1000LA 第一阶段：FDT 双核与基础外设

本文记录 `main` 基线上的第一阶段实现。目标是让同一份板级内核根据 U-Boot
提供的 DTB 启动两个 CPU，并从 DTB 发现启动和运行所需外设，而不是把板卡地址写死在
Rust 平台配置中。本轮修改先以代码审查、交叉构建、状态机测例和反汇编完成静态闭环；
在首个调度时间片问题没有形成完整因果链前不重复给板卡上电。静态结果不能替代最终的
SATA 数据完整性和 GMAC 收发验收。

正确性基线固定为 Linux 上游提交
`f9a2394a23482bfd330911e9c8295b71724feacd`：LoongArch 入口和任务切换对照
`arch/loongarch/kernel/{head,switch}.S` 与 `arch/loongarch/include/asm/stackframe.h`；
GMAC 对照 `drivers/net/ethernet/stmicro/stmmac` 的 dwmac1000 描述符、DMA、中断和
`probe -> ndo_open` 生命周期；AHCI 对照 `drivers/ata/libahci.c` 的命令发布、完成判定
以及先清端口状态、后清 HBA 状态的电平中断次序。TGOSKits 只用于观察同一板卡的固件
和外设现象，不作为寄存器语义或并发正确性的依据。

## 已实现的启动合同

- 启动入口保留 U-Boot 的 `argc/argv`，也接受 UHI 的 `a0=-2, a1=DTB` 约定；没有
  有效 DTB 时明确停止启动。
- CPU 数量和硬件 ID 来自 DTB 中状态非 `disabled` 的 `/cpus/cpu@*` 节点，当前构建
  上限为 2；逻辑 CPU ID 不再假定等于硬件 ID。
- CPU1 通过 2K1000 IOCSR mailbox 设置入口和启动栈，再由 IPI 释放。
- 早期/运行时 UART、LIOINTC、RTC、AHCI、GMAC0、重启和关机寄存器均从 DTB
  的 `compatible`、`reg`、`interrupts`、alias/phandle 或 syscon 属性获得。
- RAM 区域优先以 DTB `/memory` 为准；板载 U-Boot 的 DTB 不含该标准节点时，回退到
  构建配置中记录的固件 DRAM bank。链接和最初建立页表所需的启动窗口仍属于镜像
  ABI，不是外设探测配置。
- 板载 U-Boot 的 `bdinfo` 还把低端 DRAM bank 顶部 16 MiB 标记为 LMB 保留区；
  平台内存布局会始终排除该固件工作区。
- AHCI 根设备接受 `/chosen/bootargs` 中的 `/dev/sda`、`/dev/sdaN`、`PARTUUID=` 和
  `PARTLABEL=`。未提供 `root=` 时，仅在唯一 ext4 分区（或整盘 ext4）可判定时启动，
  不再固定选择某个分区。
- `ethernet0` alias 优先选择 GMAC0；没有 alias 时选择首个状态可用的兼容节点，明确
  忽略 `status = "disabled"` 的 GMAC1。

驱动容器仍沿用当前 `main` 的 `axdriver` 接口，但 `probe_global()` 的实例和资源均由
DTB 产生。仓库当前 vendored 依赖缺少 `axklib/rdrive`，因此本阶段没有强行升级整套
ArceOS 依赖来接入 `driver-dyn`。这不会退回固定地址，但后续依赖统一时应迁到同一套
rdrive 动态驱动模型。

## DTB 最低要求

板端 DTB 至少需要：

- 两个启用的 CPU 节点，`reg` 为固件实际使用的硬件 CPU ID；
- `/chosen/stdout-path` 指向启用的 NS16550 兼容串口；
- 至少一个启用的 `loongson,2k1000-icu`、`loongson,ls2k1000-icu` 或
  `loongson,liointc` 节点，包含两个 `reg` 窗口及 CPU HWI 2..5 的父中断；
- AHCI 节点及其 `reg`、`interrupts`，以及能表达至少 32 位 DMA 的 `dma-mask`；
- `ethernet0` alias 指向希望启用的 DWMAC 节点，并提供 MAC 地址、PHY mode、
  `phy-handle`、中断和 DMA mask；
- RTC 节点、`syscon-reboot` 和 `syscon-poweroff`（后两者缺失时只能安全停机）；
- 正确的 `/chosen/bootargs`。推荐提供 `/memory`；缺失时平台必须有与固件
  DRAM bank 一致的回退配置。

仓库中的 [静态检查用 DTS](../tests/fixtures/ls2k1000la.dts) 只用于验证解析合同，应以
板卡 U-Boot 实际导出的 DTB 为准，不应直接假定其地址和中断与所有 2K1000LA 板卡一致。

## 构建与静态验收

```bash
make kernel-la-2k1000
./scripts/check_ls2k1000_static.sh
```

产物为 `moss-2k1000la.bin`。脚本检查 DTS 可编译、正好两个 CPU 启用、GMAC1 禁用、
生产代码不包含样例中的固定外设地址、镜像具有 LoongArch MZ 头，以及 ELF 中 4 KiB
对齐的 `.dma_uncached` 段可写但不可执行。

原 QEMU LoongArch 路径必须继续通过：

```bash
./scripts/run_la.sh
```

验收标志为 cagent 和 buildstorm 两组结束标记、
`BUILDSTORM_COMPILE mode=multi ok=true`，以及 QEMU 正常关机。本检查验证 `main` 的
通用运行时/SMP 改动没有破坏 QEMU，不表示 QEMU 模拟了 2K1000LA 外设。

静态检查脚本也会在宿主机运行 46 个不访问 MMIO 的解析和驱动状态机测例，覆盖 AHCI 完成
状态、完成槽复用竞态、错误恢复准入、非 NCQ 单命令准入、块范围溢出检查、GMAC TX 环形
队列、RX 资源耗尽回收、MAC/DMA 中断屏蔽和 Clause 22 PHY 协商，以及 LIOINTC 映射与
单源分发；脚本还会检查限时等待在注册 listener 后二次判断条件，验证样例 DTB 中 UART、
AHCI、GMAC、RTC、reboot 和 poweroff 的启动合同，并从板级 ELF 反汇编确认入口关闭中断
和任务切换保存/恢复 `CSR.PRMD`。

2026-08-07 在本功能分支上的实际结果：

- `./scripts/check_ls2k1000_static.sh` 通过，当前板级镜像为 3,834,496 字节；
- RISC-V QEMU 内核交叉构建通过；
- LoongArch QEMU 两轮均报告 `cpu_nodes=12`、`online=12`，cagent 完整结束；
- 两轮 buildstorm 均为 `ok=true`，分别用时 370.81 秒和 371.39 秒，输出大小均为
  1,714,568 字节，并由 QEMU 正常关机。

## U-Boot 真机验收建议

下面地址仅是已使用过的加载窗口，不是驱动资源；若 U-Boot 内存布局不同，应选择其
允许的无冲突地址。`go` 跳转到镜像起点加 64 字节的实际入口，并把 DTB 地址作为第一个
参数传入：

```console
tftpboot 0x9000000098000000 moss-2k1000la.bin
tftpboot 0x9000000003000000 board.dtb
fdt addr 0x9000000003000000
fdt resize
fdt set /chosen bootargs "root=PARTLABEL=moss-root rw"
go 0x9000000098000040 0x9000000003000000
```

真机第一阶段至少应依次确认：两个 CPU 都进入调度器；串口仍可交互；LIOINTC 无中断
风暴；目标 SATA 分区可反复读写并在断电重启后通过文件系统检查；GMAC0 能持续双向
收发；reboot/poweroff 动作与板卡 syscon 定义一致。

当前 GMAC 探测阶段只读取能力、MAC/PHY 和注册 DT IRQ，保持 MAC、DMA 与硬件中断源
停止；第一次网络队列操作才按 Linux `ndo_open` 生命周期复位 DMA、初始化描述符并启动
收发。PBL、burst 和可选 AXI 参数来自 `snps,*` 设备树属性，未提供时使用 Linux 默认值，
不再写板级固定 AXI 参数。MAC-core 中没有对应处理函数的 RGMII/PCS/PMT/timestamp 源
保持屏蔽，硬中断只确认并屏蔽对应 DMA 队列；描述符回收和中断重开在网络轮询阶段完成，
RX 网络缓冲池暂时耗尽时也会先把描述符归还 DMA。缺少 IRQ 或注册失败时保留轮询降级
路径。链路初始速率和双工按 DT `phy-handle` 指向的 Clause 22 PHY 标准寄存器协商结果
配置；PHY 未连接或协商未完成时返回可重试错误，不再固定猜测 1000/full，也不把未解释
的 GMAC RGMII/PCS 原始状态当成速率。当前尚未实现 Linux phylib 的周期性 link-change 状态机，
运行中拔插或重新协商仍需要真机补充验收。AHCI
要求 DT 提供可翻译 IRQ，按控制器能力分配最多 32 个命令槽和独立完成状态，并让等待
槽位/完成的任务睡眠；当前读写仍使用非 NCQ ATA DMA 命令，因此按 Linux libata 规则
只允许一个命令在途，不会错误地把 HBA 多槽能力当成 NCQ。超时或致命错误会串行停止
命令引擎，在保持 FIS 接收开启的状态下执行 SATA COMRESET；任一步超时、链路未恢复或
设备持续忙时，端口保持中断屏蔽并失败关闭，不再忽略失败后继续发命令。上述状态机和
交叉构建已经静态验收，
但 QEMU 通用路径使用 VirtIO，仍不能代替真机 SATA 数据完整性、GMAC 持续收发和中断
风暴测试。
