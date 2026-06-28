# MOSS OS

MOSS 是一个基于 StarryOS/ArceOS 演进而来的 Rust 宏内核操作系统，面向 Linux 用户态兼容场景开发。当前比赛版本主要支持 RISC-V 64 与 LoongArch64，可运行 BusyBox、musl/glibc 用户程序、性能测试以及 LTP 系统调用测试。

> 初赛阶段的汇报材料位于 [`preliminary-report`](./preliminary-report/) 文件夹

## 支持情况

| 架构 | 状态 | 主要用途 |
| --- | --- | --- |
| RISC-V 64 | 支持 | 比赛测评、交互演示、内核调试 |
| LoongArch64 | 支持 | 比赛测评、交互演示、内核调试 |
| AArch64 | 基础支持 | 上游构建与启动检查 |
| x86_64 | 开发中 | 上游构建检查 |

## 运行环境

推荐使用项目比赛镜像，镜像中已经准备好 Rust 工具链、交叉编译工具和 QEMU：

```bash
docker run -it --rm --privileged \
  --platform linux/amd64 \
  -v "$(pwd):/work" \
  -w /work \
  docker.io/zhouzhouyi/os-contest:20260510 \
  /bin/bash
```

moss 在 [`Makefile`](./Makefile) 中也写入了 docker 的启动指令，可以进入 wsl 环境后

```bash
make docker
```

进入 docker 环境。

后续命令均在仓库根目录执行。LoongArch64 运行需要 QEMU 10；如果不使用 Docker，请自行安装 Rust、`cargo-binutils`、对应架构的交叉编译工具以及 QEMU。

## 构建比赛内核

进入 docker 环境后，使用以下命令可进行编译，分别构建两个比赛架构：

```bash
make kernel-rv
make kernel-la
```

也可以一次构建全部比赛产物：

```bash
make all
```

生成的比赛内核位于仓库根目录：

```text
kernel-rv
kernel-la
```

## 测评入口

### RISC-V 64

```bash
make run_rv
```

### LoongArch64

```bash
make run_la
```

上述入口会依次完成以下工作：

1. 构建对应架构的比赛内核；
2. 从基准镜像准备干净的 `sdcard-rv.img` 或 `sdcard-la.img`；
3. 启动 QEMU；
4. 由内核启动 BusyBox，并执行内嵌的 [`src/init.sh`](./src/init.sh)；
5. 准备 musl 与 glibc 运行环境；
6. 自动执行测评项目，结束后关机并退出 QEMU。

当前自动测评覆盖 basic、BusyBox、cyclictest、iozone、iperf、netperf、libcbench、libctest、lmbench、Lua 和 LTP 等测试组。LTP 用例清单位于 [`src/ltp_testcases.sh`](./src/ltp_testcases.sh)。

串口日志输出分别保存在：

```text
docs/output_rv.txt
docs/output_la.txt
```

## 进入交互式终端的方法

RISC-V 64：

```bash
make rv-shell
```

LoongArch64：

```bash
make la-shell
```

系统启动完成后会进入 BusyBox Shell ，终端应显示如下：

```text
Entering interactive shell...
starry:~#
```

进入交互式终端后可以使用常见的 Linux 命令检查系统。


Note: 交互入口会先把 4 GiB 的基础镜像复制为 `tmp-rv-shell.img` 或 `tmp-la-shell.img`，以免操作终端时污染正式测评镜像。在 Docker 挂载目录中，这一步可能耗时较长，而且复制命令默认不显示进度；内核构建结束后若暂时没有新输出，请等待镜像复制完成，镜像复制完成后会顺利进入终端界面。

输入 `exit` 可退出 Shell。如果 QEMU 没有自动关闭，可依次按下 `Ctrl+A`、`X`。

## moss 仓库测试脚本

### 1. 完整重置镜像并运行单架构测评

[`scripts/run_rv.sh`](./scripts/run_rv.sh) 和 [`scripts/run_la.sh`](./scripts/run_la.sh) 会删除旧的工作镜像和内核，从 `imgs/` 复制基准镜像，然后重新构建并启动 QEMU。这组脚本适合需要确保磁盘状态完全一致的重复测评，也可将脚本中的复制镜像指令注释以节省拷贝解压的时间。

```bash
cd scripts
./run_rv.sh
# 或
./run_la.sh
```

### 2. 自定义测例开关选项

[`src/init.sh`](./src/init.sh) 中可以选择 rv 或 la 架构是否开启测评，也可以选择每个模块的测试是否进行，设置为 1 代表启动测试，0 代表跳过测试。

```bash
RUN_RV=1
RUN_LA=1

# Test switches: 1 means run, 0 means only print group start/end.
basic=1
busybox=1
cyclictest=1
iozone=1
iperf=1
libcbench=1
libctest=1
lmbench=1
ltp=1
lua=1
netperf=1
```

`some_testcases`中若有赋值则只对选中的 LTP 测例进行测试，其他测试均跳过，方便对 LTP 测例进行本地测试。例如：

```bash
some_testcases="
access01 access01
access02 access02
"
```

[`src/ltp_testcases.sh`](./src/ltp_testcases.sh) 中写入了用于测评的所有 LTP 测例，如有注释则可跳过被注释的测例。

## 许可证

本项目使用 Apache License 2.0，详见 [`LICENSE`](./LICENSE) 和 [`NOTICE`](./NOTICE)。
