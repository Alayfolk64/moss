#import "../template.typ": evidence

= 附录 D：测试日志与结果记录模板

本附录保留为后续填入实际运行日志。当前不写固定通过数量或性能成绩。

== 建议记录字段

#table(
  columns: (3fr, 7fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*字段*], [*说明*],
  [执行日期], [例如 2026-06-09。],
  [代码版本], [git commit、dirty 状态或源码包版本。],
  [目标架构], [riscv64、loongarch64、x86_64、aarch64 等。],
  [构建命令], [完整 cargo/xtask/qemu 命令和 feature。],
  [rootfs], [musl/glibc、镜像路径、主要测试程序版本。],
  [测试组], [basic、busybox、ltp、iozone、lmbench、netperf 等。],
  [原始日志路径], [串口/QEMU/脚本输出保存位置。],
  [摘要], [由日志提取的 pass/fail/skip 或性能数据。],
)

== 已有入口

#table(
  columns: (3fr, 7fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*路径*], [*用途*],
  [`src/init.sh`], [默认测试脚本和分组输出逻辑。],
  [`src/ltp_testcases.sh`], [LTP 用例清单。],
  [`src/init_shell.sh`], [shell 调试入口。],
  [`syscalls_code/`], [系统调用测试资料和分类文档。],
  [`docs/io_benchmark_pass_optimization.md`], [仓库内已有 I/O benchmark 相关记录，可作为后续交叉参考，但本文未引用其中成绩。],
)

== 代码依据

#evidence((
  [`脚本机制`], [`src/init.sh`], [测试组变量和 `run_ltp` 输出处理逻辑。],
  [`用例注入`], [`src/main.rs`], [`LTP_CASES_RAW` 来自 `src/ltp_testcases.sh`。],
  [`测试资料`], [`syscalls_code/`], [保存系统调用测试相关资料。],
))
