# 操作指南

## 镜像和赛题

| 用途 | 来源 |
| --- | --- |
| 决赛镜像、线下赛 BuildStorm 实板镜像 | [百度网盘](https://pan.baidu.com/s/1zTb2NPCp9HE_l_M-3-oczg?pwd=kyva)，提取码 `kyva` |
| 初赛 rootfs | [官方赛题仓库 pre-2025 分支](https://github.com/oscomp/testsuits-for-oskernel/tree/pre-2025)，按该仓库 README 执行 `make docker`、`make` 构建 |

上述均为比赛官方资源。网盘文件与各架构的对应关系尚未核对。

## 构建和 QEMU

环境和编译步骤见[仓库 README](../../README.md#环境构建与运行)。MOSS 使用的 Docker 镜像是 `docker.io/zhouzhouyi/os-contest:20260510`，构建工具链为 `nightly-2025-05-20`。初赛赛题的编译环境按赛题仓库自身说明准备。

| 入口 | 当前行为 |
| --- | --- |
| `make all` | 生成 `kernel-rv`、`kernel-la`；运行时单核选择初赛，多核选择决赛 |
| `make site-rv` | 编译并启动现场 Shell；QEMU 为 16 核、8 GiB |
| `make site-la` | 编译并启动现场 Shell；QEMU 为 8 核、8 GiB |
| `make site-vf2` | 编译 VF2 实板内核，部署步骤见下方板卡指南 |
| `make site-2kla` | 编译龙芯实板内核，部署步骤见下方板卡指南 |

当前 QEMU 现场入口还存在路径不一致：`site-rv` 检查 `images/rootfs/site-rv.img`，实际加载根目录的 `alpine-linux-riscv64-ext4fs.img`；`site-la` 检查 `images/rootfs/site-la.img`，实际加载根目录的 `sdcard-la-pub.img`。本轮未修改 Makefile，不能只放好其中一份就认为准备完成。

`scripts/run_rv.sh` 和 `scripts/run_la.sh` 均使用 8 核，适用于通用内核的决赛入口；初赛必须使用单核。其余旧测试和开发验收脚本已删除，保留脚本的用法见 [scripts/README.md](../../scripts/README.md)。

## 实板和调试

- [板卡指南](boards/README.md)：双板启动、烧录、网络和维护。
- [常用命令](常用命令.md)。
- [StarryOS 模块阅读顺序](StarryOS-模块阅读顺序.md)：早期阅读笔记。
- [应用适配与调试总手册](../summaries/现场赛应用适配与调试总手册.md)：遇到具体软件问题时查阅。

还需补齐：网盘文件对应表、初赛双架构的完整启动命令。本次未重新构建或上板测试。
