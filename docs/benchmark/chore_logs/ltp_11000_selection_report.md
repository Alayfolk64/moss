# LTP 11000 分选择报告

## 结论

- 只调整了 `src/ltp_testcases.sh` 的注释状态；`src/init.sh` 未修改。
- 原始启用 LTP 用例 837 个，本次保留 535 个，注释掉 302 个，没有新增启用项。
- 已选用例都在保存的 RV 基线日志里同时具备 `ltp-musl` 和 `ltp-glibc` 的 `Summary`，未选择无 `Summary` 用例。
- RV 复跑结果：`ltp-musl` + `ltp-glibc` 的 `Summary.passed` 合计正好 11000。
- LA 复跑结果：同一套选择实际 `Summary.passed` 合计 11080。
- RV/LA 复跑都没有发现卡死或启动后缺 `Summary` 的已选用例。

## 文件

- 调整文件：`src/ltp_testcases.sh`
- 保存的调整前 RV 日志：`docs/saved_logs/20260623-030450-pre-ltp-selection/output_rv.txt`
- 复跑 RV 日志：`docs/output_rv.txt`
- 复跑 LA 日志：`docs/output_la.txt`
- 复跑 RV 保存副本：`docs/saved_logs/20260623-201553-before-conflict-fix-rerun/output_rv.txt`
- 复跑 LA 保存副本：`docs/saved_logs/20260623-201553-before-conflict-fix-rerun/output_la.txt`
- 后续 cyclictest/iozone-only 输出已另存到 `docs/saved_logs/20260623-215313-before-restoring-ltp-rerun-outputs/`；当前 `docs/output_rv.txt` 和 `docs/output_la.txt` 已恢复为本报告对应的 LTP 复跑日志。

## 选择规则

- 只统计 LTP 的 `Summary` 里的 `passed`。
- 候选用例必须在保存的 RV 基线日志中有 `ltp-musl` 和 `ltp-glibc` 两边的 `Summary`。
- 按后续要求强制保留 `fanotify14`、`splice07`、`epoll_ctl03`。
- 除强制保留项外，继续排除单边超过 500 分的 fanotify/splice 高分项。
- 当前 RV/LA 复跑日志均确认，除强制保留项外，没有其它已选 fanotify/splice 用例单边 `passed` 超过 500。
- 高分且快的用例优先保留；慢用例优先淘汰。选完后用 RV/LA 都重新跑了一遍验证。

## 强制保留与高分剔除

| 用例 | 状态 | RV musl | RV glibc | RV 合计 | LA musl | LA glibc | LA 合计 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `fanotify14` | 保留 | 513 | 513 | 1026 | 513 | 513 | 1026 |
| `splice07` | 保留 | 431 | 431 | 862 | 474 | 474 | 948 |
| `epoll_ctl03` | 保留 | 256 | 256 | 512 | 256 | 256 | 512 |
| `fanotify16` | 剔除 | 1386 | 1386 | 2772 | - | - | - |
| `fanotify10` | 剔除 | 1047 | 1047 | 2094 | - | - | - |
| `fanotify01` | 剔除 | 702 | 702 | 1404 | - | - | - |

## RV 复跑结果

| 组 | 用例数 | Summary 数 | passed | failed | broken | skipped | warnings | 耗时 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `ltp-musl` | 535 | 535 | 5496 | 30 | 1 | 143 | 0 | 10m53s |
| `ltp-glibc` | 535 | 535 | 5504 | 21 | 0 | 144 | 0 | 12m27s |
| 合计 | 1070 | 1070 | 11000 | 51 | 1 | 287 | 0 | 23m21s |

- RV 从 LTP 开始到 LTP 结束：23m21s。
- RV 从系统 boot 到 LTP 结束：23m29s。

## LA 复跑结果

| 组 | 用例数 | Summary 数 | passed | failed | broken | skipped | warnings | 耗时 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `ltp-musl` | 535 | 535 | 5534 | 32 | 1 | 126 | 0 | 12m13s |
| `ltp-glibc` | 535 | 535 | 5546 | 23 | 0 | 123 | 0 | 13m43s |
| 合计 | 1070 | 1070 | 11080 | 55 | 1 | 249 | 0 | 25m57s |

- LA 从 LTP 开始到 LTP 结束：25m57s。
- LA 从系统 boot 到 LTP 结束：26m03s。

## 总时间

- RV + LA 的 LTP 段总耗时：49m18s。
- RV + LA 从 boot 到 LTP 结束的总耗时：49m32s。

## 卡死和慢用例

- 卡死/跳过：无。
- 已选用例中没有启动后缺 `Summary` 的情况。
- 复跑中相对慢但正常完成的代表用例包括 `statx12`、`rename01`、`rename03`、`fsetxattr01`、`fsmount01`、`fsync01`、`fanotify14`、`fanotify15`、`memfd_create01`。这些都有 `Summary`，所以按规则正常计入。

## 空组说明

日志里仍能看到非 LTP 组的 start/end 标记，例如 `cyclictest-glibc`、`iozone-glibc`、`netperf-glibc` 等；这些组没有实际 benchmark 输出，也没有执行用例。本次实际执行内容是 `RUN LTP CASE ...` 的 LTP 用例。
