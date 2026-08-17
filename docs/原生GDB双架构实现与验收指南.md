# MOSS 原生 GDB 双架构实现与验收指南

本文记录 MOSS 在 RISC-V 64 和 LoongArch64 上运行目标机原生 GDB 的实现、demo、验收命令和真实结果。
这里的“原生 GDB”是指 GDB 和被调试程序都运行在 MOSS 用户空间，不是宿主机 QEMU 的 `-s/-S`，
也不是 `gdbserver`。

## 1. 当前结论

| 平台 | 架构 | 结果 | 证据 |
| --- | --- | --- | --- |
| QEMU virt | RISC-V 64 | 通过 | GDB 15.2 完成断点、运行、回溯、寄存器、内存、单步和继续；退出正常；只读 fsck 通过 |
| QEMU virt | LoongArch64 | 通过 | 同一合同通过；只读 fsck 通过 |
| VisionFive 2 | RISC-V 64 | 尚未执行实体验收 | 当前主机没有 VF2 串口设备；runner 在构建、TFTP 和分区操作前以 rc=2 停止 |
| LS2K1000LA | LoongArch64 | 尚未执行实体验收 | 当前主机没有龙芯板串口设备；runner 在构建、TFTP 和分区操作前以 rc=2 停止 |

两种实体板镜像已经从当前源码强制全量构建成功，但“能构建”不等于“实板跑通”，因此实体板一栏不能
写成通过。2026-08-17 生成的镜像为：

```text
c5ddd08a85accdc7e7c245f3b5751c7d6e852122e7cfe955e788ac45b9bc10ac  moss-vf2.bin
4c70c28792ed98d2ace765bf222a19f47867bfd9c8472dd5d97fc750c39057fd  moss-2k1000la.bin
```

## 2. 从 TGOSKits 借鉴了什么

参考实现是同级目录 `tgoskits/os/StarryOS/kernel/src/syscall/task/ptrace.rs`、两种架构的 `axcpu`
用户上下文，以及 `tgoskits/apps/starry/gdb-smoke` 和 `test-ptrace-gdb`。移植采用了这些核心思路：

- tracee 在信号、exec 和软件单步点进入可由 tracer 观察的停止态；tracer 用 `wait` 收到 `SIGCHLD`；
- RISC-V 64 和 LoongArch64 按 Linux UAPI 的寄存器布局实现 `NT_PRSTATUS`/`NT_FPREGSET`；
- 没有通用硬件单步接口时，解码当前控制流，暂时在下一条将执行的指令处写软件断点；
- 修改代码页后恢复指令并刷新指令缓存；
- GDB 读取进程映射、auxv、状态和内存所依赖的 procfs 接口必须与 ptrace 停止态一致。

MOSS 当前代码基线和 TGOSKits 的任务、VM、等待队列接口并不相同，所以不是直接复制文件。MOSS 版本
使用自己的 `PollSet`、`UserContext`、进程表和地址空间接口，并把验收范围限定为本次要求的单进程
核心调试流程。TGOSKits 中更完整的 thread/clone、seize、syscall-stop 压力合同没有在本次结果中冒充
已通过。

## 3. 内核实现

主要实现位于 `kernel/src/syscall/task/ptrace.rs`，包括：

- `PTRACE_TRACEME`、`ATTACH`、`DETACH`、`CONT`、`KILL` 和 `SINGLESTEP`；
- `PEEKTEXT/PEEKDATA`、`POKETEXT/POKEDATA`；
- `GETREGS/SETREGS`、`GETFPREGS/SETFPREGS` 和 `GETREGSET/SETREGSET`；
- `GETSIGINFO/SETSIGINFO`、`SETOPTIONS`、`GETEVENTMSG`；
- tracer/tracee 所有权检查、停止上下文保存、恢复信号、exec stop 和 `CLD_TRAPPED` 通知；
- `process_vm_readv` 和 `process_vm_writev`。

`kernel/src/task/signal.rs` 在普通信号递送前进入 ptrace stop；`kernel/src/task/user.rs` 在返回用户态前
处理待停止请求，并在软件单步断点命中后恢复原指令。`PTRACE_KILL` 会唤醒已经停住的 tracee，避免
tracee 永久睡在 ptrace 等待点。

### 3.1 架构寄存器

RISC-V 使用 Linux `user_regs_struct` 顺序导出 32 个整数寄存器和 PC；LoongArch 使用 Linux
`user_pt_regs` 顺序导出 `r0..r31`、`orig_a0`、PC 和 badv。两种架构都保存浮点上下文。

RISC-V GDB 会用不同长度的 `NT_FPREGSET` iovec 探测 FLEN。实现必须对长度不匹配返回 `EINVAL`；
如果把短缓冲区静默截断，GDB 会把 64 位浮点寄存器误判成 32 位，后续寄存器包就会错位。

### 3.2 软件单步

RISC-V 需要区分 16 位压缩指令和 32 位指令，并计算条件分支、`jal`、`jalr` 的实际下一 PC；
LoongArch64 计算普通顺序执行、条件分支、`b/bl`、`jirl` 的下一 PC。内核在下一 PC 写临时
`ebreak`/`break`，继续 tracee；陷阱到达后恢复原指令、刷新 I-cache，再把 tracee以 `SIGTRAP`
停住。这个过程保证 `stepi` 只执行一条指令，而不是直接把 PC 改到下一地址从而跳过副作用。

### 3.3 GDB 依赖的 procfs

本次同时补齐：

- `/proc/[pid]/status` 的 `TracerPid`；
- `/proc/[pid]/auxv`，内容来自 ELF loader 实际生成的 auxv；
- `/proc/[pid]/mem`，只允许拥有该停止态的 tracer 读写；
- 跨 VMA 边界的 `/proc/[pid]/mem` 访问返回已完成的部分长度，而不是把整次读取判成失败。

最后一项对 GDB 很重要：GDB 会按自身缓冲区大小读取目标内存，合法页末尾后面可能正好是未映射区；
Linux 语义是返回页内已经读取的字节。

## 4. Demo 和验收命令

demo 是 `tests/gdb-native-demo.c`。调用链固定为：

```text
main -> demo_entry -> demo_worker -> native_marker
```

`native_marker(41)` 返回 42，程序最终打印：

```text
gdb-native-demo value=42
```

板内普通编译命令是：

```sh
gcc -O0 -g3 -fno-omit-frame-pointer -fno-pie -no-pie \
    -Wall -Wextra -Werror gdb-native-demo.c -o gdb-native-demo
file gdb-native-demo
readelf -h gdb-native-demo
```

验收使用 Alpine GDB 15.2，以普通 batch 命令执行 `tests/gdb-native-smoke.gdb`：

```sh
timeout -s KILL 240 gdb -q -nx -batch \
    -x gdb-native-smoke.gdb ./gdb-native-demo
```

脚本实际执行以下 GDB 操作：

```gdb
break native_marker
run
bt
info proc mappings
info files
info auxv
info registers
x/4gx $sp
stepi
continue
```

判定通过不是只搜索最后一行。`scripts/gdb_native_workload.py` 同时要求：

- breakpoint 已设置且命中 `native_marker`；
- backtrace 同时出现 `native_marker`、`demo_worker`、`demo_entry`、`main`；
- proc mappings、ELF files、auxv、`TracerPid`、寄存器和栈内存都有输出；
- `stepi` 前后 PC 都能解析且不相等；
- demo 打印 `value=42`，GDB 报告 inferior 正常退出；
- 输出中不得出现 `Cannot access memory`、`Couldn't get registers`、ptrace ENOSYS、程序未运行等
  失败文本。

当前使用非 PIE `ET_EXEC` 是明确的兼容边界，不是关闭调试功能。LoongArch Alpine GDB 15.2 的
native PIE 启动路径在第一个用户断点前没有把主程序符号地址加上映射基址，曾尝试访问 `0x7e88` 并在
`0x898` 插断点；实际 `AT_ENTRY` 和映射基址是正确的。使用普通动态链接 `ET_EXEC` 后，断点、动态
musl、回溯、寄存器、内存和单步全部工作。PIE 支持不能依据本次结果判为通过。

## 5. QEMU 真实输出

### 5.1 RISC-V 64

完整日志：`tmp/qemu-rv-gdb-native-20260817-174414.log`。该轮先用
`apk --no-network add --allow-untrusted /var/cache/moss-gdb-native/*.apk`完成严格离线安装，再冷启动验收。
关键原文为：

```text
GNU gdb (GDB) 15.2
gcc (Alpine 14.2.0) 14.2.0
Type:                              EXEC (Executable file)
Machine:                           RISC-V

Breakpoint 1 at 0x10608: file gdb-native-demo.c, line 5.
GDB_NATIVE_BREAKPOINT_SET
Breakpoint 1, native_marker (value=41) at gdb-native-demo.c:5
GDB_NATIVE_BREAKPOINT_HIT pc=0x10608
#0  native_marker (value=41) at gdb-native-demo.c:5
#1  0x0000000000010646 in demo_worker (value=41) at gdb-native-demo.c:12
#2  0x0000000000010674 in demo_entry (value=41) at gdb-native-demo.c:18
#3  0x0000000000010690 in main () at gdb-native-demo.c:23
TracerPid:  60
pc             0x10608  0x10608 <native_marker+14>
GDB_NATIVE_REGS_DONE
0x3fffffd00:  0x0000000000000008  0x0000002900000008
GDB_NATIVE_MEMORY_DONE
GDB_NATIVE_STEPI_DONE before=0x10608 after=0x1060c
gdb-native-demo value=42
[Inferior 1 (process 60) exited normally]
GDB_NATIVE_SMOKE_DONE
[gdb-native] ALL NATIVE GDB BREAKPOINT/BACKTRACE/REGISTERS/MEMORY/STEPI/CONTINUE TESTS PASSED
[qemu-gdb] PASS working image e2fsck read-only check
```

该轮内核 SHA-256 是
`efa6091b0c47ca1fda979fd41bdba0727dc7b28ecefbdf7fbb025532c31baaa4`。

### 5.2 LoongArch64

完整日志：`tmp/qemu-la-gdb-native-20260817-174223.log`。该轮同样使用 `apk --no-network`
完成离线安装和冷启动验收。关键原文为：

```text
GNU gdb (GDB) 15.2
gcc (Alpine 14.2.0) 14.2.0
Type:                              EXEC (Executable file)
Machine:                           LoongArch

Breakpoint 1 at 0x120000718: file gdb-native-demo.c, line 5.
GDB_NATIVE_BREAKPOINT_SET
Breakpoint 1, native_marker (value=41) at gdb-native-demo.c:5
GDB_NATIVE_BREAKPOINT_HIT pc=0x120000718
#0  native_marker (value=41) at gdb-native-demo.c:5
#1  0x0000000120000770 in demo_worker (value=41) at gdb-native-demo.c:12
#2  0x00000001200007b8 in demo_entry (value=41) at gdb-native-demo.c:18
#3  0x00000001200007e8 in main () at gdb-native-demo.c:23
TracerPid:  54
r4             0x29                41
pc             0x120000718         0x120000718 <native_marker+20>
GDB_NATIVE_REGS_DONE
0x3fffffd00:  0x0000000000000008  0x0000002900000008
GDB_NATIVE_MEMORY_DONE
GDB_NATIVE_STEPI_DONE before=0x120000718 after=0x12000071c
gdb-native-demo value=42
[Inferior 1 (process 64) exited normally]
GDB_NATIVE_SMOKE_DONE
[gdb-native] ALL NATIVE GDB BREAKPOINT/BACKTRACE/REGISTERS/MEMORY/STEPI/CONTINUE TESTS PASSED
[qemu-gdb] PASS working image e2fsck read-only check
```

该轮内核 SHA-256 是
`f38afece63ed137b8b21edd9dee4e9f5ccf24b0a786913ac485ac516e505ae00`。

日志中还出现 GDB 启动时调用尚未实现的 `setfsuid/setfsgid` 告警，但 GDB 没有依赖其结果完成本合同，
所有强制标记和目标进程退出码仍通过。它们不应被描述成已经实现。

## 6. 实体板自动化和安全边界

统一入口为：

```sh
./scripts/run_hardware_gdb_native.sh vf2
./scripts/run_hardware_gdb_native.sh ls2k1000la
./scripts/run_hardware_gdb_native.sh all
```

runner 为两块板准备对应 Alpine 官方 APK 闭包和相同 demo，然后执行两轮冷启动：

1. 原厂 Linux 修复并卸载目标分区；
2. U-Boot 交接前武装 30 秒硬件看门狗，并通过 DTB 写入本轮绝对硬截止；
3. 第一轮 MOSS 只离线安装 GDB/GCC，执行 `sync` 后自动复位；
4. 原厂 Linux 对目标分区执行 `e2fsck -fn`；
5. 第二轮冷启动运行完整 GDB demo，再自动复位并做第二次只读 fsck。

VF2 使用 `/dev/mmcblk1p3`、Debian p1 SSH relay、JH7110 看门狗和哈希命名 TFTP 镜像。
LS2K1000LA 使用 `/dev/sda3`，由原厂 Linux 的 stmmac 网络把离线包写入分区；MOSS 阶段不依赖此前
未通过的 LS2K1000LA netlink 查询。两块板失败时都只进行有界同步/退出，然后等待已武装的硬件
看门狗；若自动恢复不可观察，runner 明确放弃，不要求人工复位。

当前主机的真实前置检查是：

```text
VF2 serial device is not connected
VF2_PREFLIGHT_RC=2
LS2K1000LA serial device is not connected: /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0
LS2K_PREFLIGHT_RC=2
```

因此还没有 `hardware-vf2-gdb-native-*.log` 或 `hardware-ls2k1000la-gdb-native-*.log` 可以作为通过
证据。接回串口和直连网卡后应直接运行上述入口；只有日志同时出现完整 GDB 通过标记、自动复位和
文件系统干净标记，才能把对应实体板改写成通过。

## 7. 复现入口和文件索引

QEMU 双架构完整复现：

```sh
./scripts/run_qemu_gdb_native.sh all
```

主要文件：

- `kernel/src/syscall/task/ptrace.rs`：ptrace、寄存器、内存和软件单步；
- `kernel/src/pseudofs/proc.rs`：status、auxv、mem；
- `vendor/axcpu/src/riscv/asm.rs`、`loongarch64/asm.rs`：I-cache 刷新；
- `tests/gdb-native-demo.c`：确定性 demo；
- `tests/gdb-native-smoke.gdb`：GDB 命令合同；
- `scripts/gdb_native_workload.py`：共同判定逻辑；
- `scripts/run_qemu_gdb_native.sh`：QEMU 冷安装、冷验收和只读 fsck；
- `scripts/run_hardware_gdb_native.sh`：双实体板统一入口；
- `scripts/run_vf2_gdb_native.py`、`run_ls2k1000_gdb_native.py`：看门狗、TFTP、自动恢复和 fsck。

本次明确没有验收多线程断点、`PTRACE_O_TRACECLONE`、`PTRACE_SYSCALL`、远程 `gdbserver` 和 PIE；
这些能力若后续需要，应增加独立合同和真实日志，不能由当前单进程 demo 外推。
