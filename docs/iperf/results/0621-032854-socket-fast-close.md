# iperf 结果

- 时间: `0621-032854`
- RV: `/tmp/starryos-rv-socket-fast-close.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.166006 | - (缺 6 项) |
| musl | 10.165125 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3645.44 | 1.781645 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 963.00 | 1.591900 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 5877.76 | 1.777003 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1075.20 | 1.588914 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3614.72 | 1.780896 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1075.20 | 1.645647 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3676.16 | 1.783470 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 965.00 | 1.592746 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6031.36 | 1.782683 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1054.72 | 1.580931 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3594.24 | 1.779647 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1075.20 | 1.645647 | - | - |
