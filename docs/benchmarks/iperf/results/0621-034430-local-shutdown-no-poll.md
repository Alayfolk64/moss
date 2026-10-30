# iperf 结果

- 时间: `0621-034430`
- RV: `/tmp/starryos-rv-local-shutdown-no-poll.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.210682 | - (缺 6 项) |
| musl | 10.191109 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3860.48 | 1.793808 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 976.00 | 1.597336 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6696.96 | 1.804281 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1054.72 | 1.580931 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3747.84 | 1.788678 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1075.20 | 1.645647 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3727.36 | 1.786444 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1034.24 | 1.620011 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6584.32 | 1.800933 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 980.00 | 1.548980 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3696.64 | 1.785751 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1085.44 | 1.648990 | - | - |
