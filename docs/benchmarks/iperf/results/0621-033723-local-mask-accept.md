# iperf 结果

- 时间: `0621-033723`
- RV: `/tmp/starryos-rv-local-mask-accept.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.249317 | - (缺 6 项) |
| musl | 10.292045 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3860.48 | 1.793808 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1034.24 | 1.620011 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6799.36 | 1.807229 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1034.24 | 1.572633 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3962.88 | 1.800145 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1105.92 | 1.655490 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3870.72 | 1.794354 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1034.24 | 1.620011 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6871.04 | 1.809240 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1126.40 | 1.607599 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4003.84 | 1.802190 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1116.16 | 1.658651 | - | - |
