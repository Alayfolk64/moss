# iperf 结果

- 时间: `0621-124747`
- RV: `/tmp/no_rv_iperf.log`
- LA: `/tmp/starryos-la-udp-recv-front-fastpath.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 6 项) | 6.383257 |
| musl | - (缺 6 项) | 6.295550 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 726.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 456.00 | 1.138158 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 828.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 478.00 | 1.075314 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 823.00 | 1.037667 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 439.00 | 1.132118 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 851.00 | 1.064630 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 511.00 | 1.230920 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 407.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 378.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 448.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 267.00 | 1.000000 |
