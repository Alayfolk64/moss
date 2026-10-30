# iperf 结果

- 时间: `0621-035909`
- RV: `/tmp/starryos-rv-socket-close-table-fast.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.273460 | - (缺 6 项) |
| musl | 10.261724 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 4085.76 | 1.805177 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1024.00 | 1.616211 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 7106.56 | 1.815562 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1095.68 | 1.596598 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4055.04 | 1.804688 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1044.48 | 1.635225 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 4218.88 | 1.811324 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1054.72 | 1.627389 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6932.48 | 1.810931 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 998.00 | 1.557114 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4014.08 | 1.802695 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1095.68 | 1.652271 | - | - |
