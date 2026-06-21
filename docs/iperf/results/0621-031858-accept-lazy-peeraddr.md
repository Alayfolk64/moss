# iperf 结果

- 时间: `0621-031912`
- RV: `docs/output_rv.txt`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.200914 | - (缺 6 项) |
| musl | 10.214600 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3686.40 | 1.784071 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 998.00 | 1.606212 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6318.08 | 1.792545 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1075.20 | 1.588914 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3717.12 | 1.786932 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1064.96 | 1.642240 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3829.76 | 1.792154 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 978.00 | 1.598160 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6553.60 | 1.800000 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1085.44 | 1.592792 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3758.08 | 1.789254 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1064.96 | 1.642240 | - | - |
