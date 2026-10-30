# iperf 结果

- 时间: `0621-030143`
- RV: `/tmp/starryos-rv-loopback-listener.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.193185 | - (缺 6 项) |
| musl | 10.173600 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3635.20 | 1.781030 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 991.00 | 1.603431 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6195.20 | 1.788430 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1064.96 | 1.584961 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3706.88 | 1.786343 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1085.44 | 1.648990 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3624.96 | 1.780411 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1024.00 | 1.616211 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6092.80 | 1.784874 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 992.00 | 1.554435 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3747.84 | 1.788678 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1085.44 | 1.648990 | - | - |
