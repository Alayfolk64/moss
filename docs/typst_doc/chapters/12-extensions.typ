#import "../template.typ": evidence

= 用户态与测试环境支持

== 用户态运行时

源码中能确认的用户态入口是 BusyBox shell。默认 `CMDLINE` 为 `/musl/busybox sh -c INIT_SCRIPT`，同时 `entry.rs` 对 BusyBox 路径做 `/musl`、`/glibc`、`/bin` fallback。`loader.rs` 在解析动态链接器时也会在 `/glibc` 和 `/musl` 前缀下 fallback，因此文档可确认系统针对 musl/glibc rootfs 做了路径兼容处理。

`src/init.sh` 会根据 rootfs 中是否存在 LoongArch 或 RISC-V 的 glibc loader 来设置 `/lib` 链接，并在 musl/glibc runtime 间切换。脚本中提到 basic、busybox、iozone、libcbench、lmbench、ltp、lua、netperf 等测试组，但是否通过要以实际日志为准，本文不写固定成绩。

== 脚本与测试组

`src/init.sh` 使用一组 shell 变量控制测试组是否运行：

#table(
  columns: (2fr, 8fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*变量*], [*当前默认值和含义*],
  [`iozone=1`], [默认启用 iozone 文件系统性能/行为测试组。],
  [`libcbench=1`], [默认启用 libcbench。],
  [`lmbench=1`], [默认启用 lmbench。],
  [`lua=1`], [默认启用 lua 相关测试。],
  [`netperf=1`], [默认启用 netperf。],
  [`basic/busybox/cyclictest/iperf/libctest/ltp=0`], [默认关闭，除非脚本变量或 `some_testcases` 改变。],
)

`src/main.rs` 把 `src/ltp_testcases.sh` 的非空非注释行拼成 `LTP_CASES` 环境变量。`init.sh` 中的 `run_ltp` 函数会读取该变量或脚本文件，并在缺少 LTP Summary 时根据 TPASS/TFAIL/TBROK/TCONF/TWARN 输出合成 summary。

== rootfs 兼容辅助

`init.sh` 创建的 wrapper 和静态配置主要是为了让测试程序找到预期路径：

- `/etc/passwd`、`/etc/group`、`/etc/resolv.conf`。
- `/lib/modules/10.0.0/build/.config` 中的若干 `CONFIG_*` 项。
- `useradd/userdel/groupdel` 的简单脚本。
- `mkfs.ext3`、`mkfs.ext4` 等 wrapper 或 no-op。
- `/bin` 下 BusyBox 命令链接。

这些内容属于用户态适配层，不应和内核功能本身混淆。

== 代码依据

#evidence((
  [`BusyBox init`], [`src/main.rs`], [`CMDLINE` 使用 `/musl/busybox sh -c INIT_SCRIPT`，feature 选择不同 init 脚本。],
  [`路径 fallback`], [`kernel/src/entry.rs`, `kernel/src/mm/loader.rs`], [init 可在 musl/glibc/bin BusyBox 间 fallback，动态链接器可在 `/glibc`、`/musl` 前缀下解析。],
  [`测试脚本`], [`src/init.sh`], [脚本准备 rootfs、运行测试组并包含 LTP summary 处理函数。],
  [`LTP 用例`], [`src/main.rs`, `src/ltp_testcases.sh`], [`build_ltp_cases_env` 把用例列表写入 `LTP_CASES` 环境变量。],
))
