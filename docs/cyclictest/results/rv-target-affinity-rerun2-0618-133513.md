# cyclictest 计分结果

- 生成时间: `2026-06-18 13:35:13`
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

- rv_target_affinity_rerun2: `docs/cyclictest/results/rv-target-affinity-rerun2-0618-133513.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv_target_affinity_rerun2 | musl | 4 | 0 | 5.669596 |
| rv_target_affinity_rerun2 | glibc | 0 | 4 | 0.000000 |

## rv_target_affinity_rerun2 / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | 2043.000000 | 1.000000 |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | 20500.000000 | 1.000000 |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | 421.000000 | 1.678626 |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | 125.000000 | 1.990970 |

## rv_target_affinity_rerun2 / glibc

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | - | - |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | - | - |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | - | - |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | - | - |
