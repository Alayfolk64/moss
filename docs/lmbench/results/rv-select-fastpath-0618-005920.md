# lmbench 计分结果

- 生成时间: `2026-06-18 00:59:20`
- baseline 已按 `608-性能测试.pdf` 第 23-26 页与官方 lmbench judge 脚本交叉核对。
- 计分项: `36`，单个 libc 理论分数范围为 `0` 到 `< 72`。
- 低延迟项: `result <= baseline` 时得 `2 - result / baseline`，否则得 `1.0`。
- 高吞吐项: `result >= baseline` 时得 `2 - baseline / result`，否则得 `1.0`。
- 缺失或非正数结果得 `0`；单项达到 baseline 得 `1.0`，极限趋近 `2.0`。
- 官方 judge 通过名称是否含 `microseconds`/`seconds` 判断方向；因此 8 个 `fs latency` 项实际按“越大越好”计分。
- `File /var/tmp/XXX write bandwidth` 和 `Protection fault` 不在 baseline 中，不计分。

## 计分规则

lmbench 共 36 个计分项，每项独立计算，最后直接求和。

低延迟项的名称包含 `microseconds` 或 `seconds`，数值越小越好：

```text
result <= baseline: score = 2 - result / baseline
result >  baseline: score = 1
```

其余项目按高吞吐项处理，数值越大越好：

```text
result >= baseline: score = 2 - baseline / result
result <  baseline: score = 1
```

- 缺失、解析失败或结果小于等于 0 时得 0 分。
- 达到 baseline 时得 1 分；性能优于 baseline 后分数逐渐接近 2 分，但不会达到 2 分。
- 单个 libc 的 36 项总分范围为 0 到小于 72 分。
- 官方 judge 仅根据项目名称判断方向，所以 8 个 `fs latency` 项虽然是延迟指标，实际仍按“越大越好”计分。

## 输入

- rv-select-fastpath: `docs/lmbench/results/rv-select-fastpath-0618-005920.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv-select-fastpath | glibc | 1 | 35 | 1.774209 |
| rv-select-fastpath | musl | 36 | 0 | 49.267509 |

## rv-select-fastpath / glibc

| 项目 | 方向（judge） | baseline | result | score |
|---|---|---:|---:|---:|
| lmbench Simple syscall:(microseconds) | 越低越好 | 9.250130 | 2.088600 | 1.774209 |
| lmbench Simple read:(microseconds) | 越低越好 | 16.819200 | - | - |
| lmbench Simple write:(microseconds) | 越低越好 | 16.875530 | - | - |
| lmbench Simple stat:(microseconds) | 越低越好 | 425.256500 | - | - |
| lmbench Simple fstat:(microseconds) | 越低越好 | 29.546740 | - | - |
| lmbench Simple open/close:(microseconds) | 越低越好 | 501.235200 | - | - |
| lmbench Select on 100 fd's:(microseconds) | 越低越好 | 56.055610 | - | - |
| lmbench Signal handler installation:(microseconds) | 越低越好 | 18.849480 | - | - |
| lmbench Signal handler overhead:(microseconds) | 越低越好 | 25.890640 | - | - |
| lmbench Pipe latency:(microseconds) | 越低越好 | 141.910600 | - | - |
| lmbench Process fork+exit:(microseconds) | 越低越好 | 4409.188000 | - | - |
| lmbench Process fork+execve:(microseconds) | 越低越好 | 2004.333300 | - | - |
| lmbench Process fork+/bin/sh -c:(microseconds) | 越低越好 | 920010.200000 | - | - |
| lmbench Pagefaults on /var/tmp/XXX:(microseconds) | 越低越好 | 978.237464 | - | - |
| lmbench lat_mmap:(microseconds) | 越低越好 | 4109.000000 | - | - |
| lmbench fs latency 0k create | 越高越好 | 24573.000000 | - | - |
| lmbench fs latency 0k remove | 越高越好 | 30703.000000 | - | - |
| lmbench fs latency 1k create | 越高越好 | 15130.000000 | - | - |
| lmbench fs latency 1k remove | 越高越好 | 255558.000000 | - | - |
| lmbench fs latency 4k create | 越高越好 | 13265.000000 | - | - |
| lmbench fs latency 4k remove | 越高越好 | 126595.000000 | - | - |
| lmbench fs latency 10k create | 越高越好 | 11723.000000 | - | - |
| lmbench fs latency 10k remove | 越高越好 | 23688.000000 | - | - |
| lmbench Pipe bandwidth:(MB/sec) | 越高越好 | 127.244000 | - | - |
| lmbench bw_file_rd io_only | 越高越好 | 71703.700000 | - | - |
| lmbench bw_file_rd open2close | 越高越好 | 2637.494000 | - | - |
| lmbench bw_mmap_rd mmap_only | 越高越好 | 1037.403000 | - | - |
| lmbench bw_mmap_rd open2close | 越高越好 | 187.020800 | - | - |
| lmbench context switch 2:(microseconds) | 越低越好 | 41.240000 | - | - |
| lmbench context switch 4:(microseconds) | 越低越好 | 41.580710 | - | - |
| lmbench context switch 8:(microseconds) | 越低越好 | 41.042310 | - | - |
| lmbench context switch 16:(microseconds) | 越低越好 | 40.770770 | - | - |
| lmbench context switch 24:(microseconds) | 越低越好 | 43.863850 | - | - |
| lmbench context switch 32:(microseconds) | 越低越好 | 47.749230 | - | - |
| lmbench context switch 64:(microseconds) | 越低越好 | 8703.026000 | - | - |
| lmbench context switch 96:(microseconds) | 越低越好 | 2952.140000 | - | - |

## rv-select-fastpath / musl

| 项目 | 方向（judge） | baseline | result | score |
|---|---|---:|---:|---:|
| lmbench Simple syscall:(microseconds) | 越低越好 | 9.250130 | 2.050800 | 1.778295 |
| lmbench Simple read:(microseconds) | 越低越好 | 16.819200 | 7.241900 | 1.569427 |
| lmbench Simple write:(microseconds) | 越低越好 | 16.875530 | 7.449800 | 1.558544 |
| lmbench Simple stat:(microseconds) | 越低越好 | 425.256500 | 17.884500 | 1.957944 |
| lmbench Simple fstat:(microseconds) | 越低越好 | 29.546740 | 3.534700 | 1.880369 |
| lmbench Simple open/close:(microseconds) | 越低越好 | 501.235200 | 75.172200 | 1.850026 |
| lmbench Select on 100 fd's:(microseconds) | 越低越好 | 56.055610 | 22.633900 | 1.596224 |
| lmbench Signal handler installation:(microseconds) | 越低越好 | 18.849480 | 4.720600 | 1.749563 |
| lmbench Signal handler overhead:(microseconds) | 越低越好 | 25.890640 | 13.296000 | 1.486455 |
| lmbench Pipe latency:(microseconds) | 越低越好 | 141.910600 | 168.372500 | 1.000000 |
| lmbench Process fork+exit:(microseconds) | 越低越好 | 4409.188000 | 2062.556500 | 1.532214 |
| lmbench Process fork+execve:(microseconds) | 越低越好 | 2004.333300 | 2127.325900 | 1.000000 |
| lmbench Process fork+/bin/sh -c:(microseconds) | 越低越好 | 920010.200000 | 124267.888900 | 1.864928 |
| lmbench Pagefaults on /var/tmp/XXX:(microseconds) | 越低越好 | 978.237464 | 19.125400 | 1.980449 |
| lmbench lat_mmap:(microseconds) | 越低越好 | 4109.000000 | 239.000000 | 1.941835 |
| lmbench fs latency 0k create | 越高越好 | 24573.000000 | 2213.000000 | 1.000000 |
| lmbench fs latency 0k remove | 越高越好 | 30703.000000 | 5377.000000 | 1.000000 |
| lmbench fs latency 1k create | 越高越好 | 15130.000000 | 1828.000000 | 1.000000 |
| lmbench fs latency 1k remove | 越高越好 | 255558.000000 | 3933.000000 | 1.000000 |
| lmbench fs latency 4k create | 越高越好 | 13265.000000 | 1828.000000 | 1.000000 |
| lmbench fs latency 4k remove | 越高越好 | 126595.000000 | 3898.000000 | 1.000000 |
| lmbench fs latency 10k create | 越高越好 | 11723.000000 | 1520.000000 | 1.000000 |
| lmbench fs latency 10k remove | 越高越好 | 23688.000000 | 3317.000000 | 1.000000 |
| lmbench Pipe bandwidth:(MB/sec) | 越高越好 | 127.244000 | 337.140000 | 1.622578 |
| lmbench bw_file_rd io_only | 越高越好 | 71703.700000 | 1704.190000 | 1.000000 |
| lmbench bw_file_rd open2close | 越高越好 | 2637.494000 | 1335.460000 | 1.000000 |
| lmbench bw_mmap_rd mmap_only | 越高越好 | 1037.403000 | 15020.970000 | 1.930936 |
| lmbench bw_mmap_rd open2close | 越高越好 | 187.020800 | 187.840000 | 1.004361 |
| lmbench context switch 2:(microseconds) | 越低越好 | 41.240000 | 79.560000 | 1.000000 |
| lmbench context switch 4:(microseconds) | 越低越好 | 41.580710 | 78.590000 | 1.000000 |
| lmbench context switch 8:(microseconds) | 越低越好 | 41.042310 | 79.360000 | 1.000000 |
| lmbench context switch 16:(microseconds) | 越低越好 | 40.770770 | 79.070000 | 1.000000 |
| lmbench context switch 24:(microseconds) | 越低越好 | 43.863850 | 80.610000 | 1.000000 |
| lmbench context switch 32:(microseconds) | 越低越好 | 47.749230 | 81.060000 | 1.000000 |
| lmbench context switch 64:(microseconds) | 越低越好 | 8703.026000 | 80.390000 | 1.990763 |
| lmbench context switch 96:(microseconds) | 越低越好 | 2952.140000 | 80.900000 | 1.972596 |

未计分但成功解析的项目:
- `lmbench File /var/tmp/XXX write bandwidth:(KB/sec)` = `12063.0`
- `lmbench Protection fault:(microseconds)` = `1.8941`
