# iperf 结果

- 时间: `0621-031508`
- RV: `/tmp/starryos-rv-udp-no-route-wake.log`
- LA: `/tmp/starryos-la-udp-no-route-wake.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.186703 | 6.772957 |
| musl | 10.209785 | 6.747160 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3594.24 | 1.778535 | 843.00 | 1.055753 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 963.00 | 1.591900 | 510.00 | 1.229412 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6225.92 | 1.789474 | 933.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1064.96 | 1.584961 | 520.00 | 1.150000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3706.88 | 1.786343 | 842.00 | 1.059382 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1105.92 | 1.655490 | 528.00 | 1.278409 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3604.48 | 1.779164 | 825.00 | 1.035152 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1024.00 | 1.616211 | 508.00 | 1.226378 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6000.64 | 1.781570 | 936.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1085.44 | 1.592792 | 505.00 | 1.124752 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3676.16 | 1.784558 | 846.00 | 1.063830 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1105.92 | 1.655490 | 542.00 | 1.297048 |
