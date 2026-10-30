# iperf 结果

- 时间: `0621-125228`
- RV: `/tmp/no_rv_iperf.log`
- LA: `/tmp/starryos-la-local-send-no-periodic-poll.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 6 项) | 6.168122 |
| musl | - (缺 6 项) | 6.667501 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 647.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 370.00 | 1.000000 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 792.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 414.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 732.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 458.00 | 1.168122 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 813.00 | 1.020910 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 485.00 | 1.189691 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 937.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 504.00 | 1.123016 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 819.00 | 1.032967 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 545.00 | 1.300917 |
