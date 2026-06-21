# iperf 结果

- 时间: `0621-031211`
- RV: `/tmp/starryos-rv-udp-no-route-wake.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.186703 | - (缺 6 项) |
| musl | 10.209785 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3594.24 | 1.778535 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 963.00 | 1.591900 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6225.92 | 1.789474 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1064.96 | 1.584961 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3706.88 | 1.786343 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1105.92 | 1.655490 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3604.48 | 1.779164 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1024.00 | 1.616211 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6000.64 | 1.781570 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1085.44 | 1.592792 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3676.16 | 1.784558 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1105.92 | 1.655490 | - | - |
