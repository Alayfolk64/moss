# MossOS

## 项目简介

MossOS 是使用 Rust 实现的 Linux ABI 兼容宏内核，基于 ArceOS 组件化框架并由 StarryOS 演化而来。系统面向操作系统内核功能、性能和跨平台能力验证，内核主体可在 RISC-V64 与 LoongArch64 两种指令集、QEMU 与物理开发板共四个平台间复用。

MossOS 已实现进程与线程、虚拟内存、文件系统、信号、网络、Futex、多核调度和 Linux 系统调用等核心能力，可运行 BusyBox、musl/glibc 用户程序及多类 Linux 应用。平台层提供统一的中断、时钟、串口和设备访问接口，并已接入 VirtIO、SD/MMC、AHCI SATA 和以太网等设备。

## 文档索引

- [决赛设计文档](决赛文档.pdf)
- [BuildStorm 内核设计优化实现文档](BuildStorm内核优化实现文档.pdf)
- [初赛汇报材料](https://pan.baidu.com/s/1Ls1hawu1NluVZ0SAmK7Izg?pwd=moss)（提取码：`moss`，包含演示视频、汇报 PPT 和初赛文档）
- [决赛答辩材料](https://pan.baidu.com/s/1ppQOjb5W-m1yjWBFRqJ3FQ?pwd=moss)（提取码: `moss`,包含答辩视频、答辩 PPT）

## 支持情况

| 平台 | 架构与配置 | 主要硬件路径 | 验证情况 |
| --- | --- | --- | --- |
| QEMU virt | RISC-V64，8 核、8 GiB | SBI、PLIC、VirtIO MMIO | 通过 CAgent 与 BuildStorm 线上测试 |
| QEMU virt | LoongArch64，12 核、8 GiB | EIOINTC、IOCSR IPI、VirtIO PCI | 通过 CAgent 与 BuildStorm 线上测试 |
| VisionFive 2 | RISC-V64，4 核、4 GiB | PLIC、UART、SD/MMC、GMAC1 | 完成实板启动、存储、网络和用户程序验证 |
| LS2K1000LA | LoongArch64，2 核、2 GiB | LIOINTC、UART、AHCI SATA、GMAC、RTC | 完成实板启动并通过现场赛测试 |

## 成绩情况

### 初赛线上阶段

截至2026年6月30日，MossOS 以总分 2601.18 位列初赛线上排行榜第 3 名。

<p align="center">
  <img src="docs/report_final/pictrues/初赛排名.png" alt="MossOS 初赛线上阶段排名" width="100%">
</p>

### 决赛线上阶段

截至2026年8月12日，MossOS 以总分 758.2 位列决赛线上排行榜第 2 名。RISC-V64 与 LoongArch64 的 BuildStorm 得分均为 180.0，CAgent 得分均为 199.1。

<p align="center">
  <img src="docs/report_final/pictrues/rank-812.png" alt="MossOS 决赛线上阶段排名" width="100%">
</p>

## 环境、构建与运行

推荐使用项目比赛镜像，其中已准备 Rust 工具链、交叉编译工具和 QEMU：

```bash
docker run -it --rm --privileged \
  --platform linux/amd64 \
  -v "$(pwd):/work" \
  -w /work \
  docker.io/zhouzhouyi/os-contest:20260510 \
  /bin/bash
```

也可以在仓库根目录直接执行：

```bash
make docker
```

不使用 Docker 时，需要自行安装 Rust nightly、`cargo-binutils`、对应架构的交叉编译工具和 QEMU；LoongArch64 需要 QEMU 10。以下命令均在仓库根目录执行。

### QEMU 构建与普通运行

分别构建两个 QEMU 平台内核，或一次构建二者：

```bash
make kernel-rv     # 生成 kernel-rv
make kernel-la     # 生成 kernel-la
make all           # 生成以上两个内核
```

使用单核配置快速启动系统：

```bash
make run_rv
make run_la
```

上述快捷入口会自动准备对应的 `sdcard-rv.img` 或 `sdcard-la.img`，运行日志分别写入 `docs/output_rv.txt` 和 `docs/output_la.txt`。

需要进入交互式 BusyBox Shell 时，可使用：

```bash
make rv-shell
make la-shell
```

输入 `exit` 可退出 Shell；若 QEMU 未自动关闭，可依次按下 `Ctrl+A`、`X`。

### 决赛线上测试复现

默认构建会嵌入 `src/init_final.sh`，启动后执行 CAgent 与 BuildStorm。先准备磁盘镜像，再使用决赛配置运行：

```bash
# RISC-V64：8 核、8 GiB
make prepare-sdcard-rv
./scripts/run_rv.sh

# LoongArch64：12 核、8 GiB
make prepare-sdcard-la
./scripts/run_la.sh
```

运行脚本使用 QEMU 快照模式，不会把测试过程写回基准磁盘镜像。初赛回归测试由 `src/init.sh` 提供，交互模式由 `src/init_shell.sh` 提供。

### 实板构建

VisionFive 2：

```bash
make kernel-vf2
```

构建产物为 `moss-vf2.bin`，目标配置为 4 核、4 GiB。

LS2K1000LA：

```bash
make kernel-la-2k1000
make kernel-la-2k1000-shell
```

对应产物为 `moss-2k1000la.bin` 和交互式镜像 `moss-2k1000la-shell.bin`，目标配置为 2 核、1 GiB。实板镜像需结合开发板的 U-Boot、TFTP 和根文件系统环境部署。
