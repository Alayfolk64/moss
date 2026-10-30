# 性能测试

[测试及计分子项](性能测试及其子项.md)介绍各项测试。每个目录将报告、`get_result.py` 和 `results/` 放在一起。

| 测试 | 报告 |
| --- | --- |
| iozone | [I/O 优化策略](iozone/优化策略_new.md)、[cache retention 专项](iozone/cache_retention_pruned_report.md) |
| lmbench | [优化报告](lmbench/优化报告.md) |
| libcbench | [优化报告](libcbench/优化报告.md) |
| cyclictest | [优化报告](cyclictest/优化报告.md) |
| iperf | [优化过程](iperf/optimization_process.md) |
| netperf | [优化过程](netperf/optimization_process.md) |

I/O 通用优化保留 2026-06-19 的策略整理，替代 6 月 9 日的总结和 6 月 18 日的旧报告；6 月 24 日的 cache retention 是独立专项。lmbench 保留 6 月 18 日报告，替代第一轮记录。

早期 LTP 选例、跳过建议和专项复测材料保留在 [chore_logs](chore_logs)，不作为当前测试配置。`results/` 中不同轮次的数据和 Ubuntu 对照结果均保留。
