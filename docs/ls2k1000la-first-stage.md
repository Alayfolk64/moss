# LS2K1000LA 第一阶段：FDT 双核与基础外设

本文记录 `main` 基线上的第一阶段实现。目标是让同一份板级内核根据 U-Boot
提供的 DTB 启动两个 CPU，并从 DTB 发现启动和运行所需外设，而不是把板卡地址写死在
Rust 平台配置中。由于当前没有开发板，本阶段的结论仅包括代码审查、交叉构建和静态
检查；不能替代真机启动、SATA 数据完整性和 GMAC 收发验收。

参考实现固定为 TGOSKits 提交
`e04580c5c06868035fa39d3ca63baf179e9fcdca`，主要参考其 LoongArch 启动参数保留、
LIOINTC、AHCI 和 GMAC 初始化次序，没有直接复制其全局固定地址配置。

## 已实现的启动合同

- 启动入口保留 U-Boot 的 `argc/argv`，也接受 UHI 的 `a0=-2, a1=DTB` 约定；没有
  有效 DTB 时明确停止启动。
- CPU 数量和硬件 ID 来自 DTB 中状态非 `disabled` 的 `/cpus/cpu@*` 节点，当前构建
  上限为 2；逻辑 CPU ID 不再假定等于硬件 ID。
- CPU1 通过 2K1000 IOCSR mailbox 设置入口和启动栈，再由 IPI 释放。
- 早期/运行时 UART、LIOINTC、RTC、AHCI、GMAC0、重启和关机寄存器均从 DTB
  的 `compatible`、`reg`、`interrupts`、alias/phandle 或 syscon 属性获得。
- RAM 区域最终以 DTB `/memory` 为准；链接和最初建立页表所需的启动窗口仍属于镜像
  ABI，不是外设探测配置。
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
- 正确的 `/memory` 和 `/chosen/bootargs`。

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

2026-08-07 在本功能分支上的实际结果：

- `./scripts/check_ls2k1000_static.sh` 通过，板级镜像为 3,797,632 字节；
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

当前 GMAC 数据面采用轮询收发，DTB 中的 IRQ 已解析并记录，但还没有启用 DMA 中断
完成/错误处理。AHCI 同样沿用 `simple-ahci` 的轮询完成路径。因此它们适合第一阶段
连通性和根文件系统验证，不应被描述为已经完成中断模式或高负载真机验收。
