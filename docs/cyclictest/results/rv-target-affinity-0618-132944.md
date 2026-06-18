# cyclictest 计分结果

- 生成时间: `2026-06-18 13:29:44`
- baseline 和公式来自官方 `judge_cyclictest-musl.py` / `judge_cyclictest-glibc.py`。
- 官方解析只记录 `end: success` 的测试段，且必须有 `kill hackbench: success`；否则整组为 0 分。
- 每段 result 为该段所有 `T:` 行的 `max(Max, Min)` 平均值，单位 `microseconds`，越低越好。

## 计分规则

```text
result <= baseline: score = 2 - result / baseline
result >  baseline: score = 1
missing/result <= 0: score = 0
```

## 输入

- rv_target_affinity: `docs/cyclictest/results/rv-target-affinity-0618-132944.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv_target_affinity | musl | 4 | 0 | 5.718204 |
| rv_target_affinity | glibc | 0 | 4 | 0.000000 |

## rv_target_affinity / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | 10039.000000 | 1.000000 |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | 1083.000000 | 1.304934 |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | 589.000000 | 1.550382 |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | 1898.000000 | 1.862889 |

## rv_target_affinity / glibc

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | - | - |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | - | - |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | - | - |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | - | - |
