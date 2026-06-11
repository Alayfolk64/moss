#import "../template.typ": evidence

= 测试与验证机制

== 当前已有入口

当前仓库中可确认的测试入口主要是用户态 init 脚本和系统调用测试资料：

#table(
  columns: (3fr, 7fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*路径*], [*用途*],
  [`src/init.sh`], [默认启动脚本，准备 rootfs、选择测试组、运行 benchmark/LTP 并输出分组标记。],
  [`src/init_shell.sh`], [`shell-init` feature 下的交互 shell 入口。],
  [`src/ltp_testcases.sh`], [LTP 用例清单，由 `src/main.rs` 注入 `LTP_CASES` 环境变量。],
  [`syscalls_code/`], [系统调用测试用例和分类资料目录。],
  [`docs/typst_doc`], [本文档工程；验证方式为 Typst 编译和路径证据检查。],
)

本文档不记录测试通过数量、性能排名或分数，因为这些需要从实际运行日志确认。若后续要补充结果，应把日志路径、执行日期、架构和 rootfs 一并写入附录。

== init.sh 的输出组织

`src/init.sh` 使用形如 `#### OS COMP TEST GROUP START ltp-$1 ####` 的文本标记组织测试组输出。`run_ltp` 对 LTP 输出做 summary 检测：如果原始输出包含 Summary 区块，则截取并输出；否则根据结果行中的 `TPASS`、`TFAIL`、`TBROK`、`TCONF`、`TWARN` 分类合成统计。

这说明测试报告可以从串口或 QEMU 日志中按 group marker 提取，但源码本身不等同于测试结果。

== 文档自身验证

本文档生成后需要验证：

1. `main.typ` 中所有 `#include` 路径存在。
2. `typst compile main.typ build/main.pdf` 能成功生成 PDF。
3. 文档不包含未替换的模板占位符。
4. 所有“代码依据”中的路径都能在当前仓库找到。
5. 文档中没有“完整支持”“X11/Wayland”“真实网卡驱动”等无源码证据的强断言。

== 代码依据

#evidence((
  [`测试入口`], [`src/init.sh`], [脚本内定义 basic/busybox/iozone/libcbench/lmbench/ltp/lua/netperf 等测试组开关。],
  [`LTP 注入`], [`src/main.rs`], [`build_ltp_cases_env` 读取 `src/ltp_testcases.sh` 并生成 `LTP_CASES`。],
  [`系统调用资料`], [`syscalls_code/`], [目录中保存系统调用测试分类和用例资料，可作为后续测试附录来源。],
))
