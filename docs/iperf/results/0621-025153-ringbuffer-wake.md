# iperf 结果

- 时间: `0621-025330`
- RV: `/tmp/starryos-rv-ringbuffer-wake.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.211926 | - (缺 6 项) |
| musl | 10.194087 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3799.04 | 1.790473 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 997.00 | 1.605817 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6400.00 | 1.795200 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1064.96 | 1.584961 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3768.32 | 1.789827 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1075.20 | 1.645647 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3737.60 | 1.787029 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 933.00 | 1.578778 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6277.12 | 1.791191 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1095.68 | 1.596598 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3860.48 | 1.794844 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1075.20 | 1.645647 | - | - |
