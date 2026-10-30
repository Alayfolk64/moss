# iperf 结果

- 时间: `0621-034017`
- RV: `/tmp/starryos-rv-local-mask-accept.log`
- LA: `/tmp/starryos-la-local-mask-accept.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.249317 | 6.761685 |
| musl | 10.292045 | 6.785221 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3860.48 | 1.793808 | 827.00 | 1.037485 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1034.24 | 1.620011 | 509.00 | 1.227898 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6799.36 | 1.807229 | 933.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1034.24 | 1.572633 | 516.00 | 1.143411 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3962.88 | 1.800145 | 840.00 | 1.057143 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1105.92 | 1.655490 | 541.00 | 1.295749 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3870.72 | 1.794354 | 832.00 | 1.043269 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1034.24 | 1.620011 | 515.00 | 1.236893 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6871.04 | 1.809240 | 928.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1126.40 | 1.607599 | 519.00 | 1.148362 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4003.84 | 1.802190 | 858.00 | 1.076923 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1116.16 | 1.658651 | 529.00 | 1.279773 |
