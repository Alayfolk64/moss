# iperf 结果

- 时间: `0621-114826`
- RV: `/tmp/starryos-rv-lookup-tcp-direct.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 9.868355 | - (缺 6 项) |
| musl | 10.268474 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3768.32 | 1.788765 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 975.00 | 1.596923 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 3102.72 | 1.577558 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 980.00 | 1.548980 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3440.64 | 1.769810 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 921.00 | 1.586319 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3983.36 | 1.800169 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 979.00 | 1.598570 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6635.52 | 1.802469 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1105.92 | 1.600333 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4065.28 | 1.805179 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1126.40 | 1.661754 | - | - |
