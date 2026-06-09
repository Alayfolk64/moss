#import "../template.typ": evidence

= 附录 A：构建与运行入口

== Cargo workspace

顶层 `Cargo.toml` 定义 workspace 成员为 `kernel`，顶层 package 名为 `starryos`，二进制入口为 `src/main.rs`。`[[bin]]` 中声明该二进制需要 `qemu` feature，因此常规 QEMU 构建应显式启用 `--features qemu` 或使用项目已有脚本/xtask 包装。

可确认的顶层 feature：

#table(
  columns: (2fr, 8fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*Feature*], [*作用*],
  [`qemu`], [启用 `axfeat/defplat`、`axfeat/bus-pci` 和 `starry-kernel/net`。],
  [`shell-init`], [选择 `src/init_shell.sh`，并注入 `STARRY_SHELL_MODE=1`。],
  [`timerfd-debug-init`], [`src/main.rs` 中引用 timerfd 调试 init 脚本；当前仓库未包含对应脚本文件，启用该 feature 前需要补齐。],
  [`smp`], [启用 `axfeat/smp`。],
  [`vf2`], [启用 `axfeat/driver-sdmmc`，并在 `src/main.rs` 中引入 VisionFive2 平台 crate。],
)

内核 crate feature：

#table(
  columns: (2fr, 8fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*Feature*], [*作用*],
  [`net`], [启用 `axfeat/net` 和可选依赖 `axnet`。],
  [`input`], [启用 `axinput` 和 `axfeat/input`，暴露 `/dev/input` 设备目录。],
  [`dev-log`], [启用 `/dev/log` 绑定路径。],
  [`memtrack`], [启用 axalloc tracking、dwarf/backtrace 相关依赖和 `/dev/memtrack`。],
)

== 文档构建

本文档的 Typst 入口是 `docs/typst_doc/main.typ`。在该目录执行：

```sh
typst compile main.typ build/main.pdf
```

生成结果为 `docs/typst_doc/build/main.pdf`。文档工程没有使用 LaTeX 构建链。

== 代码依据

#evidence((
  [`workspace`], [`Cargo.toml`], [`members = ["kernel"]`，`[[bin]] name = "starryos"`，required feature 为 `qemu`。],
  [`内核 feature`], [`kernel/Cargo.toml`], [`net/input/dev-log/memtrack` 定义可选能力和依赖。],
  [`文档入口`], [`docs/typst_doc/main.typ`], [Typst `#include` 多章节文件并导入 `template.typ`。],
))
