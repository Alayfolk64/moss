# iperf 结果

- 时间: `0621-040453`
- RV: `/tmp/starryos-rv-local-sendrecv-borrow.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.314908 | - (缺 6 项) |
| musl | 10.274774 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 4034.56 | 1.802705 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1054.72 | 1.627389 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 7157.76 | 1.816881 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1126.40 | 1.607599 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3993.60 | 1.801683 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1116.16 | 1.658651 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 4136.96 | 1.807588 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1034.24 | 1.620011 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6871.04 | 1.809240 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1054.72 | 1.580931 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4198.40 | 1.811357 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1075.20 | 1.645647 | - | - |
