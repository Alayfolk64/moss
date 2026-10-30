# iperf 结果

- 时间: `0621-125352`
- RV: `/tmp/no_rv_iperf.log`
- LA: `/tmp/starryos-la-local-send-no-periodic-poll-2.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 6 项) | 6.690204 |
| musl | - (缺 6 项) | 6.702317 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 827.00 | 1.037485 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 505.00 | 1.221782 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 924.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 503.00 | 1.121272 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 813.00 | 1.025830 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 532.00 | 1.283835 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 828.00 | 1.038647 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 488.00 | 1.194672 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 936.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 514.00 | 1.140078 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 839.00 | 1.056019 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 524.00 | 1.272901 |
