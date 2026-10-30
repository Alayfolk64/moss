# lmbench 计分结果

- 生成时间: `2026-06-13 15:36:04`
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

- ubuntu-glibc: `D:\QQ_info\glibc_lmbench.attempt2.log`
- ubuntu-musl: `D:\QQ_info\musl_lmbench.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| ubuntu-glibc | glibc | 36 | 0 | 49.616777 |
| ubuntu-musl | musl | 36 | 0 | 50.927688 |

## ubuntu-glibc / glibc

| 项目 | 方向（judge） | baseline | result | score |
|---|---|---:|---:|---:|
| lmbench Simple syscall:(microseconds) | 越低越好 | 9.250130 | 1.723400 | 1.813689 |
| lmbench Simple read:(microseconds) | 越低越好 | 16.819200 | 2.518500 | 1.850260 |
| lmbench Simple write:(microseconds) | 越低越好 | 16.875530 | 2.225700 | 1.868111 |
| lmbench Simple stat:(microseconds) | 越低越好 | 425.256500 | 10.438300 | 1.975454 |
| lmbench Simple fstat:(microseconds) | 越低越好 | 29.546740 | 3.851100 | 1.869661 |
| lmbench Simple open/close:(microseconds) | 越低越好 | 501.235200 | 21.121600 | 1.957861 |
| lmbench Select on 100 fd's:(microseconds) | 越低越好 | 56.055610 | 18.974700 | 1.661502 |
| lmbench Signal handler installation:(microseconds) | 越低越好 | 18.849480 | 3.322800 | 1.823719 |
| lmbench Signal handler overhead:(microseconds) | 越低越好 | 25.890640 | 29.650100 | 1.000000 |
| lmbench Pipe latency:(microseconds) | 越低越好 | 141.910600 | 160.772700 | 1.000000 |
| lmbench Process fork+exit:(microseconds) | 越低越好 | 4409.188000 | 4050.909800 | 1.081257 |
| lmbench Process fork+execve:(microseconds) | 越低越好 | 2004.333300 | 3477.500000 | 1.000000 |
| lmbench Process fork+/bin/sh -c:(microseconds) | 越低越好 | 920010.200000 | 23733.404300 | 1.974203 |
| lmbench Pagefaults on /var/tmp/XXX:(microseconds) | 越低越好 | 978.237464 | 3.752500 | 1.996164 |
| lmbench lat_mmap:(microseconds) | 越低越好 | 4109.000000 | 359.000000 | 1.912631 |
| lmbench fs latency 0k create | 越高越好 | 24573.000000 | 3956.000000 | 1.000000 |
| lmbench fs latency 0k remove | 越高越好 | 30703.000000 | 6965.000000 | 1.000000 |
| lmbench fs latency 1k create | 越高越好 | 15130.000000 | 2876.000000 | 1.000000 |
| lmbench fs latency 1k remove | 越高越好 | 255558.000000 | 4853.000000 | 1.000000 |
| lmbench fs latency 4k create | 越高越好 | 13265.000000 | 2743.000000 | 1.000000 |
| lmbench fs latency 4k remove | 越高越好 | 126595.000000 | 5021.000000 | 1.000000 |
| lmbench fs latency 10k create | 越高越好 | 11723.000000 | 2269.000000 | 1.000000 |
| lmbench fs latency 10k remove | 越高越好 | 23688.000000 | 4090.000000 | 1.000000 |
| lmbench Pipe bandwidth:(MB/sec) | 越高越好 | 127.244000 | 191.570000 | 1.335783 |
| lmbench bw_file_rd io_only | 越高越好 | 71703.700000 | 951.270000 | 1.000000 |
| lmbench bw_file_rd open2close | 越高越好 | 2637.494000 | 836.240000 | 1.000000 |
| lmbench bw_mmap_rd mmap_only | 越高越好 | 1037.403000 | 5910.160000 | 1.824471 |
| lmbench bw_mmap_rd open2close | 越高越好 | 187.020800 | 740.680000 | 1.747501 |
| lmbench context switch 2:(microseconds) | 越低越好 | 41.240000 | 69.500000 | 1.000000 |
| lmbench context switch 4:(microseconds) | 越低越好 | 41.580710 | 79.620000 | 1.000000 |
| lmbench context switch 8:(microseconds) | 越低越好 | 41.042310 | 130.560000 | 1.000000 |
| lmbench context switch 16:(microseconds) | 越低越好 | 40.770770 | 137.930000 | 1.000000 |
| lmbench context switch 24:(microseconds) | 越低越好 | 43.863850 | 140.670000 | 1.000000 |
| lmbench context switch 32:(microseconds) | 越低越好 | 47.749230 | 157.380000 | 1.000000 |
| lmbench context switch 64:(microseconds) | 越低越好 | 8703.026000 | 163.970000 | 1.981159 |
| lmbench context switch 96:(microseconds) | 越低越好 | 2952.140000 | 167.240000 | 1.943350 |

未计分但成功解析的项目:
- `lmbench File /var/tmp/XXX write bandwidth:(KB/sec)` = `49109.0`

## ubuntu-musl / musl

| 项目 | 方向（judge） | baseline | result | score |
|---|---|---:|---:|---:|
| lmbench Simple syscall:(microseconds) | 越低越好 | 9.250130 | 1.726400 | 1.813365 |
| lmbench Simple read:(microseconds) | 越低越好 | 16.819200 | 2.646600 | 1.842644 |
| lmbench Simple write:(microseconds) | 越低越好 | 16.875530 | 2.284300 | 1.864638 |
| lmbench Simple stat:(microseconds) | 越低越好 | 425.256500 | 10.692600 | 1.974856 |
| lmbench Simple fstat:(microseconds) | 越低越好 | 29.546740 | 4.106800 | 1.861007 |
| lmbench Simple open/close:(microseconds) | 越低越好 | 501.235200 | 21.626300 | 1.956854 |
| lmbench Select on 100 fd's:(microseconds) | 越低越好 | 56.055610 | 18.696500 | 1.666465 |
| lmbench Signal handler installation:(microseconds) | 越低越好 | 18.849480 | 3.376100 | 1.820892 |
| lmbench Signal handler overhead:(microseconds) | 越低越好 | 25.890640 | 17.632300 | 1.318970 |
| lmbench Pipe latency:(microseconds) | 越低越好 | 141.910600 | 129.960000 | 1.084212 |
| lmbench Process fork+exit:(microseconds) | 越低越好 | 4409.188000 | 1710.062500 | 1.612159 |
| lmbench Process fork+execve:(microseconds) | 越低越好 | 2004.333300 | 1847.290300 | 1.078352 |
| lmbench Process fork+/bin/sh -c:(microseconds) | 越低越好 | 920010.200000 | 20946.500000 | 1.977232 |
| lmbench Pagefaults on /var/tmp/XXX:(microseconds) | 越低越好 | 978.237464 | 2.901400 | 1.997034 |
| lmbench lat_mmap:(microseconds) | 越低越好 | 4109.000000 | 220.000000 | 1.946459 |
| lmbench fs latency 0k create | 越高越好 | 24573.000000 | 5094.000000 | 1.000000 |
| lmbench fs latency 0k remove | 越高越好 | 30703.000000 | 8829.000000 | 1.000000 |
| lmbench fs latency 1k create | 越高越好 | 15130.000000 | 3518.000000 | 1.000000 |
| lmbench fs latency 1k remove | 越高越好 | 255558.000000 | 6196.000000 | 1.000000 |
| lmbench fs latency 4k create | 越高越好 | 13265.000000 | 3523.000000 | 1.000000 |
| lmbench fs latency 4k remove | 越高越好 | 126595.000000 | 6185.000000 | 1.000000 |
| lmbench fs latency 10k create | 越高越好 | 11723.000000 | 2915.000000 | 1.000000 |
| lmbench fs latency 10k remove | 越高越好 | 23688.000000 | 5484.000000 | 1.000000 |
| lmbench Pipe bandwidth:(MB/sec) | 越高越好 | 127.244000 | 254.190000 | 1.499414 |
| lmbench bw_file_rd io_only | 越高越好 | 71703.700000 | 1202.520000 | 1.000000 |
| lmbench bw_file_rd open2close | 越高越好 | 2637.494000 | 1129.280000 | 1.000000 |
| lmbench bw_mmap_rd mmap_only | 越高越好 | 1037.403000 | 8474.630000 | 1.877587 |
| lmbench bw_mmap_rd open2close | 越高越好 | 187.020800 | 887.040000 | 1.789163 |
| lmbench context switch 2:(microseconds) | 越低越好 | 41.240000 | 59.590000 | 1.000000 |
| lmbench context switch 4:(microseconds) | 越低越好 | 41.580710 | 70.070000 | 1.000000 |
| lmbench context switch 8:(microseconds) | 越低越好 | 41.042310 | 111.220000 | 1.000000 |
| lmbench context switch 16:(microseconds) | 越低越好 | 40.770770 | 112.230000 | 1.000000 |
| lmbench context switch 24:(microseconds) | 越低越好 | 43.863850 | 113.110000 | 1.000000 |
| lmbench context switch 32:(microseconds) | 越低越好 | 47.749230 | 113.900000 | 1.000000 |
| lmbench context switch 64:(microseconds) | 越低越好 | 8703.026000 | 114.890000 | 1.986799 |
| lmbench context switch 96:(microseconds) | 越低越好 | 2952.140000 | 119.310000 | 1.959585 |

未计分但成功解析的项目:
- `lmbench File /var/tmp/XXX write bandwidth:(KB/sec)` = `77386.0`
- `lmbench Protection fault:(microseconds)` = `0.6637`
