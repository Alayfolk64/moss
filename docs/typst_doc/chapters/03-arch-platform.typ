#import "../template.typ": evidence

= 启动与初始化

== 顶层入口

`src/main.rs` 是 `starryos` 二进制入口。它根据 feature 选择内置脚本：

#table(
  columns: (2.5fr, 3fr, 4.5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*条件*], [*脚本*], [*用途*],
  [`timerfd-debug-init`], [`include_str!("init_timerfd_debug.sh")`], [源码声明了 timerfd 调试入口，但当前 `src/` 目录未包含该脚本，启用前需要补齐文件。],
  [`shell-init`], [`src/init_shell.sh`], [交互 shell 模式，并设置 `STARRY_SHELL_MODE=1`。],
  [默认], [`src/init.sh`], [准备 rootfs 环境、运行选定测试组。],
)

默认命令行为 `["/musl/busybox", "sh", "-c", INIT_SCRIPT]`。`src/main.rs` 还读取 `src/ltp_testcases.sh`，过滤空行和注释后写入 `LTP_CASES` 环境变量，作为 init 脚本中 LTP 用例选择的来源。

== 内核 init 流程

`kernel/src/entry.rs::init` 是用户 init 进程的创建路径：

```text
pseudofs::mount_all()
spawn_alarm_task()
resolve_init_command()
new_user_aspace_empty()
copy_from_kernel()
load_user_app()
UserContext::new(entry, stack, 0)
new_user_task()
Process::new_init()
N_TTY.bind_to()
ProcessData::new()
add_stdio()
spawn_task()
task.join()
unmount_all() / flush() / system_off()
```

该流程说明 StarryOS 的第一个用户任务来自 ELF 装载，而不是把 shell 逻辑放在内核中。`resolve_init_command` 对 BusyBox 路径做 fallback，可在 `/musl/busybox`、`/glibc/busybox`、`/bin/busybox` 间尝试。

== Rootfs 初始化脚本

`src/init.sh` 负责在用户态准备测试环境。脚本做的事情包括：

- 创建 `/bin` 并调用 BusyBox `--install -s /bin`。
- 根据 rootfs 中动态链接器存在情况，设置 musl/glibc 相关 `/lib` 和 `/lib64` 链接。
- 创建 `/etc/passwd`、`/etc/group`、`/etc/resolv.conf`。
- 写入 `/lib/modules/10.0.0/build/.config` 中若干测试期望项，例如 `CONFIG_EVENTFD=y`、`CONFIG_USER_NS=y`、`CONFIG_NET_NS=y`。
- 放置 `useradd/userdel/groupdel` 和若干 `mkfs.*` wrapper，满足测试程序对工具名的依赖。
- 通过变量开关选择 basic、busybox、iozone、libcbench、lmbench、ltp、lua、netperf 等测试组。

这些脚本动作是测试环境适配，不等同于内核实现了所有 Linux 功能。例如 `.config` 文件是用户态测试探测用的伪配置文件，不能作为完整功能证明。

== 架构配置

`kernel/src/config/mod.rs` 根据 `target_arch` 选择 `riscv64`、`loongarch64`、`x86_64`、`aarch64` 的常量文件。当前文件中可确认的公共配置包括：

#table(
  columns: (2.5fr, 2.5fr, 5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*常量*], [*典型值*], [*用途*],
  [`KERNEL_STACK_SIZE`], [`0x4_0000`], [创建用户任务时传给 `TaskInner::new` 的内核栈大小。],
  [`USER_SPACE_BASE`], [`0x1000`], [用户地址空间起点。],
  [`USER_STACK_TOP`], [`0x4_0000_0000` 或 `0x7fff_0000_0000`], [用户栈顶，随架构配置不同。],
  [`USER_HEAP_BASE`], [`0x4000_0000`], [初始用户堆起点，`brk` 使用。],
  [`USER_INTERP_BASE`], [`0x400_0000`], [动态链接器装载基址。],
  [`SIGNAL_TRAMPOLINE`], [`0x6000_1000`], [信号 trampoline 用户映射地址。],
)

从代码证据看，本文只描述这些配置常量和 feature 对平台的影响，不展开具体启动固件、链接脚本或板级设备初始化细节。

== 代码依据

#evidence((
  [`二进制入口`], [`src/main.rs`], [`CMDLINE` 使用 BusyBox shell 执行内置 init 脚本，并调用 `starry_kernel::entry::init`。],
  [`init 进程`], [`kernel/src/entry.rs`], [`mount_all`、`load_user_app`、`new_user_task`、`Process::new_init`、`add_stdio`、`spawn_task` 串联启动流程。],
  [`脚本入口`], [`src/init.sh`], [用户态准备 `/bin`、动态链接器、账户文件、测试工具 wrapper 和测试组开关。],
  [`架构常量`], [`kernel/src/config/*.rs`], [按 `target_arch` 选择用户空间、栈、堆和 signal trampoline 地址。],
))
