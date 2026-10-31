# 操作指南

## 镜像和赛题

| 用途 | 来源 |
| --- | --- |
| 决赛镜像、线下赛 BuildStorm 实板镜像 | [百度网盘](https://pan.baidu.com/s/1zTb2NPCp9HE_l_M-3-oczg?pwd=kyva)，提取码 `kyva` |
| 初赛 rootfs | [官方赛题仓库 pre-2025 分支](https://github.com/oscomp/testsuits-for-oskernel/tree/pre-2025)，按该仓库 README 执行 `make docker`、`make` 构建 |

上述均为比赛官方资源。

## 构建和 QEMU

以下命令在 `moss` 根目录执行。运行 `make docker` 进入项目构建环境，环境配置见[仓库 README](../../README.md#环境构建与运行)。初赛赛题按赛题仓库的 README 准备环境。

| 命令 | 用途 |
| --- | --- |
| `make all` | 生成 `kernel-rv`、`kernel-la`；运行时单核选择初赛，多核选择决赛 |
| `make site-rv` | 编译并启动 RISC-V64 QEMU Shell，16 核、8 GiB |
| `make site-la` | 编译并启动 LoongArch64 QEMU Shell，8 核、8 GiB |
| `make site-vf2` | 编译 VF2 实板内核，生成 `build/site-vf2/moss-vf2.bin` |
| `make site-2kla` | 编译龙芯实板内核，生成 `build/site-2kla/moss-2kla.bin` |

运行 QEMU Shell 前，需要准备以下镜像路径，均相对于 `moss` 根目录。同一行的两个路径应指向同一份解压后的镜像，可用软链接共用。

| 命令 | Makefile 检查路径 | QEMU 加载路径 |
| --- | --- | --- |
| `make site-rv` | `images/rootfs/site-rv.img` | `alpine-linux-riscv64-ext4fs.img` |
| `make site-la` | `images/rootfs/site-la.img` | `sdcard-la-pub.img` |

## 初赛与决赛测试

将对应阶段的镜像解压后放到 `moss` 根目录，RISC-V64 命名为 `sdcard-rv.img`，LoongArch64 命名为 `sdcard-la.img`。切换初赛、决赛时，换成对应阶段的镜像。

| 阶段 | 架构 | 运行命令 | QEMU 配置 |
| --- | --- | --- | --- |
| 初赛 | RISC-V64 | `bash scripts/run_pre_rv.sh` | 1 核、1 GiB |
| 初赛 | LoongArch64 | `bash scripts/run_pre_la.sh` | 1 核、1 GiB |
| 决赛 | RISC-V64 | `bash scripts/run_final_rv.sh` | 8 核、16 GiB |
| 决赛 | LoongArch64 | `bash scripts/run_final_la.sh` | 8 核、8 GiB |

脚本会先编译对应架构的内核，再启动 QEMU。同一架构的初赛和决赛脚本只在 CPU 核数与内存配置上不同，内核按实际核数选择测试入口。

终端显示运行输出，日志分别保存到 `docs/output_rv.txt`、`docs/output_la.txt`，再次运行同架构脚本会覆盖日志。QEMU 使用快照模式，退出后不回写镜像。日志统计用法见[脚本说明](../../scripts/README.md)。

## 实板启动与烧录

- 星光二号板（VisionFive 2）：[启动指令](boards/VF2开发板启动命令.md)、[SD 卡烧录教程](boards/星光二号板烧录教程.md)。
- 龙芯二号板（LS2K1000LA）：[启动指令](boards/2K1000LA板-启动指令.md)、[分区烧录教程](boards/龙芯二号板烧录教程.md)。

网卡、串口和磁盘设备名的使用说明见[板卡指南](boards/README.md)。
