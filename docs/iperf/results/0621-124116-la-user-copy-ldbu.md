# iperf 结果

- 时间: `0621-124117`
- RV: `/tmp/no_rv_iperf.log`
- LA: `/tmp/starryos-la-user-copy-ldbu.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 6 项) | 6.327821 |
| musl | - (缺 6 项) | 6.701848 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 814.00 | 1.022113 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 497.00 | 1.209256 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 667.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 477.00 | 1.073375 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 693.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 390.00 | 1.023077 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 791.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 499.00 | 1.212425 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 899.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 521.00 | 1.151631 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 842.00 | 1.059382 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 528.00 | 1.278409 |
