# lmbench 计分结果

- 生成时间: `2026-06-17 22:34:41`
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

- rv-interrupted-0617-223407: `docs/lmbench/results/rv-interrupted-0617-223407.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv-interrupted-0617-223407 | glibc | 19 | 17 | 25.433115 |
| rv-interrupted-0617-223407 | musl | 36 | 0 | 46.051155 |

## rv-interrupted-0617-223407 / glibc

| 项目 | 方向（judge） | baseline | result | score |
|---|---|---:|---:|---:|
| lmbench Simple syscall:(microseconds) | 越低越好 | 9.250130 | 2.609000 | 1.717950 |
| lmbench Simple read:(microseconds) | 越低越好 | 16.819200 | 10.081000 | 1.400625 |
| lmbench Simple write:(microseconds) | 越低越好 | 16.875530 | 12.371800 | 1.266879 |
| lmbench Simple stat:(microseconds) | 越低越好 | 425.256500 | 54.602600 | 1.871601 |
| lmbench Simple fstat:(microseconds) | 越低越好 | 29.546740 | 36.416100 | 1.000000 |
| lmbench Simple open/close:(microseconds) | 越低越好 | 501.235200 | 124.492000 | 1.751630 |
| lmbench Select on 100 fd's:(microseconds) | 越低越好 | 56.055610 | 83.776700 | 1.000000 |
| lmbench Signal handler installation:(microseconds) | 越低越好 | 18.849480 | 6.649200 | 1.647248 |
| lmbench Signal handler overhead:(microseconds) | 越低越好 | 25.890640 | 18.253600 | 1.294973 |
| lmbench Pipe latency:(microseconds) | 越低越好 | 141.910600 | 217.684800 | 1.000000 |
| lmbench Process fork+exit:(microseconds) | 越低越好 | 4409.188000 | 5974.694700 | 1.000000 |
| lmbench Process fork+execve:(microseconds) | 越低越好 | 2004.333300 | 6179.651400 | 1.000000 |
| lmbench Process fork+/bin/sh -c:(microseconds) | 越低越好 | 920010.200000 | 369788.000000 | 1.598061 |
| lmbench Pagefaults on /var/tmp/XXX:(microseconds) | 越低越好 | 978.237464 | 26.672200 | 1.972734 |
| lmbench lat_mmap:(microseconds) | 越低越好 | 4109.000000 | 364.000000 | 1.911414 |
| lmbench fs latency 0k create | 越高越好 | 24573.000000 | 1213.000000 | 1.000000 |
| lmbench fs latency 0k remove | 越高越好 | 30703.000000 | 3453.000000 | 1.000000 |
| lmbench fs latency 1k create | 越高越好 | 15130.000000 | 722.000000 | 1.000000 |
| lmbench fs latency 1k remove | 越高越好 | 255558.000000 | 2364.000000 | 1.000000 |
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

未计分但成功解析的项目:
- `lmbench File /var/tmp/XXX write bandwidth:(KB/sec)` = `16873.0`
- `lmbench Protection fault:(microseconds)` = `3.6788`

## rv-interrupted-0617-223407 / musl

| 项目 | 方向（judge） | baseline | result | score |
|---|---|---:|---:|---:|
| lmbench Simple syscall:(microseconds) | 越低越好 | 9.250130 | 2.722800 | 1.705647 |
| lmbench Simple read:(microseconds) | 越低越好 | 16.819200 | 13.699300 | 1.185496 |
| lmbench Simple write:(microseconds) | 越低越好 | 16.875530 | 12.727200 | 1.245819 |
| lmbench Simple stat:(microseconds) | 越低越好 | 425.256500 | 54.937900 | 1.870812 |
| lmbench Simple fstat:(microseconds) | 越低越好 | 29.546740 | 38.268400 | 1.000000 |
| lmbench Simple open/close:(microseconds) | 越低越好 | 501.235200 | 131.306700 | 1.738034 |
| lmbench Select on 100 fd's:(microseconds) | 越低越好 | 56.055610 | 91.277800 | 1.000000 |
| lmbench Signal handler installation:(microseconds) | 越低越好 | 18.849480 | 5.831300 | 1.690639 |
| lmbench Signal handler overhead:(microseconds) | 越低越好 | 25.890640 | 18.858900 | 1.271594 |
| lmbench Pipe latency:(microseconds) | 越低越好 | 141.910600 | 198.119100 | 1.000000 |
| lmbench Process fork+exit:(microseconds) | 越低越好 | 4409.188000 | 2940.182100 | 1.333169 |
| lmbench Process fork+execve:(microseconds) | 越低越好 | 2004.333300 | 2908.750000 | 1.000000 |
| lmbench Process fork+/bin/sh -c:(microseconds) | 越低越好 | 920010.200000 | 290668.000000 | 1.684060 |
| lmbench Pagefaults on /var/tmp/XXX:(microseconds) | 越低越好 | 978.237464 | 22.661700 | 1.976834 |
| lmbench lat_mmap:(microseconds) | 越低越好 | 4109.000000 | 290.000000 | 1.929423 |
| lmbench fs latency 0k create | 越高越好 | 24573.000000 | 1096.000000 | 1.000000 |
| lmbench fs latency 0k remove | 越高越好 | 30703.000000 | 5289.000000 | 1.000000 |
| lmbench fs latency 1k create | 越高越好 | 15130.000000 | 799.000000 | 1.000000 |
| lmbench fs latency 1k remove | 越高越好 | 255558.000000 | 4228.000000 | 1.000000 |
| lmbench fs latency 4k create | 越高越好 | 13265.000000 | 777.000000 | 1.000000 |
| lmbench fs latency 4k remove | 越高越好 | 126595.000000 | 4212.000000 | 1.000000 |
| lmbench fs latency 10k create | 越高越好 | 11723.000000 | 590.000000 | 1.000000 |
| lmbench fs latency 10k remove | 越高越好 | 23688.000000 | 4362.000000 | 1.000000 |
| lmbench Pipe bandwidth:(MB/sec) | 越高越好 | 127.244000 | 292.490000 | 1.564963 |
| lmbench bw_file_rd io_only | 越高越好 | 71703.700000 | 1381.140000 | 1.000000 |
| lmbench bw_file_rd open2close | 越高越好 | 2637.494000 | 1042.620000 | 1.000000 |
| lmbench bw_mmap_rd mmap_only | 越高越好 | 1037.403000 | 10378.090000 | 1.900039 |
| lmbench bw_mmap_rd open2close | 越高越好 | 187.020800 | 155.880000 | 1.000000 |
| lmbench context switch 2:(microseconds) | 越低越好 | 41.240000 | 98.190000 | 1.000000 |
| lmbench context switch 4:(microseconds) | 越低越好 | 41.580710 | 94.500000 | 1.000000 |
| lmbench context switch 8:(microseconds) | 越低越好 | 41.042310 | 96.100000 | 1.000000 |
| lmbench context switch 16:(microseconds) | 越低越好 | 40.770770 | 91.800000 | 1.000000 |
| lmbench context switch 24:(microseconds) | 越低越好 | 43.863850 | 95.060000 | 1.000000 |
| lmbench context switch 32:(microseconds) | 越低越好 | 47.749230 | 94.560000 | 1.000000 |
| lmbench context switch 64:(microseconds) | 越低越好 | 8703.026000 | 97.590000 | 1.988787 |
| lmbench context switch 96:(microseconds) | 越低越好 | 2952.140000 | 100.850000 | 1.965838 |

未计分但成功解析的项目:
- `lmbench File /var/tmp/XXX write bandwidth:(KB/sec)` = `6313.0`
- `lmbench Protection fault:(microseconds)` = `2.7097`
