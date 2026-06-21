# iperf 结果

- 时间: `0621-121112`
- RV: `/tmp/no_rv_iperf.log`
- LA: `/tmp/starryos-la-user-copy-word.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 6 项) | 6.075243 |
| musl | - (缺 6 项) | 6.084135 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 668.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 389.00 | 1.000000 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 702.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 406.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 651.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 412.00 | 1.075243 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 554.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 376.00 | 1.000000 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 723.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 390.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 721.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 416.00 | 1.084135 |
