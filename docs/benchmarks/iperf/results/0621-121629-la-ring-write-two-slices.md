# iperf 结果

- 时间: `0621-121629`
- RV: `/tmp/no_rv_iperf.log`
- LA: `/tmp/starryos-la-ring-write-two-slices.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 6 项) | 6.316225 |
| musl | - (缺 6 项) | 6.276276 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 734.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 438.00 | 1.102740 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 826.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 449.00 | 1.015590 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 758.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 475.00 | 1.197895 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 673.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 438.00 | 1.102740 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 809.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 429.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 736.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 461.00 | 1.173536 |
