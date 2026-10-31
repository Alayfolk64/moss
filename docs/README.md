# 文档

环境配置和编译方法见[仓库 README](../README.md#环境构建与运行)，镜像准备和运行命令见[操作指南](guides/README.md)。

| 目录 | 内容 |
| --- | --- |
| [操作指南](guides/README.md) | 镜像来源、初赛与决赛 QEMU 测试、VF2 和龙芯板的启动与烧录 |
| [设计报告](reports/README.md) | 初赛、决赛、BuildStorm 和现场赛的 PDF 与 Typst 源文件 |
| [性能测试](benchmarks/README.md) | iozone、lmbench、libcbench、cyclictest、iperf、netperf 的优化报告、计分脚本和结果 |
| [问题与修复记录](history/README.md) | 板卡驱动、串口、文件系统、页表和多核调度的问题定位与修复 |
| [测试日志与采样](evidence/README.md) | 测试输出、性能采样和实验数据 |

## 常用入口

- [初赛与决赛 QEMU 测试](guides/README.md#初赛与决赛测试)
- 星光二号板（VisionFive 2）：[启动指令](guides/boards/VF2开发板启动命令.md)、[SD 卡烧录](guides/boards/星光二号板烧录教程.md)
- 龙芯二号板（LS2K1000LA）：[启动指令](guides/boards/2K1000LA板-启动指令.md)、[分区烧录](guides/boards/龙芯二号板烧录教程.md)
- [运行与日志统计脚本](../scripts/README.md)

板卡教程中的网卡、串口和磁盘设备名使用原测试环境的配置，执行前请替换为所用电脑和板卡的对应设备名。
