# iperf 结果

- 时间: `0621-123646`
- RV: `/tmp/no_rv_iperf.log`
- LA: `/tmp/starryos-la-user-copy-aligned-dword.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 6 项) | 6.000000 |
| musl | - (缺 6 项) | 6.000000 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 615.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 312.00 | 1.000000 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 606.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 284.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 645.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 344.00 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | - | - | 502.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | - | - | 264.00 | 1.000000 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | - | - | 384.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | - | - | 308.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | - | - | 603.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | - | - | 370.00 | 1.000000 |
