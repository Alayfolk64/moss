# cyclictest 计分结果

- 生成时间: `2026-06-23 21:46:28`
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

- rv: `docs/output_rv.txt`
- la: `docs/output_la.txt`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv | musl | 4 | 0 | 4.000000 |
| rv | glibc | 4 | 0 | 4.587540 |
| la | musl | 4 | 0 | 4.431794 |
| la | glibc | 4 | 0 | 4.795381 |

## rv / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | 8529.000000 | 1.000000 |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | 8500.875000 | 1.000000 |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | 2298000.000000 | 1.000000 |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | 1698727.500000 | 1.000000 |

## rv / glibc

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | 768.000000 | 1.587540 |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | 3047.875000 | 1.000000 |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | 2117341.000000 | 1.000000 |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | 1269504.875000 | 1.000000 |

## la / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | 1058.000000 | 1.431794 |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | 5488.375000 | 1.000000 |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | 1557197.000000 | 1.000000 |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | 900505.000000 | 1.000000 |

## la / glibc

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| cyclictest NO_STRESS_P1 (microseconds) | 1862.000000 | 381.000000 | 1.795381 |
| cyclictest NO_STRESS_P8 (microseconds) | 1558.125000 | 2532.375000 | 1.000000 |
| cyclictest STRESS_P1 (microseconds) | 1310.000000 | 1588215.000000 | 1.000000 |
| cyclictest STRESS_P8 (microseconds) | 13842.750000 | 910264.250000 | 1.000000 |
