# iperf 结果

- 时间: `0621-032212`
- RV: `/tmp/starryos-rv-accept-lazy-peeraddr.log`
- LA: `docs/output_la.txt`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.200914 | 6.691899 |
| musl | 10.214600 | 6.480766 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3686.40 | 1.784071 | 825.00 | 1.035152 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 998.00 | 1.606212 | 501.00 | 1.215569 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6318.08 | 1.792545 | 920.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1075.20 | 1.588914 | 500.00 | 1.116000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3717.12 | 1.786932 | 825.00 | 1.040000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1064.96 | 1.642240 | 533.00 | 1.285178 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3829.76 | 1.792154 | 748.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 978.00 | 1.598160 | 471.00 | 1.165605 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6553.60 | 1.800000 | 860.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1085.44 | 1.592792 | 467.00 | 1.053533 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3758.08 | 1.789254 | 771.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1064.96 | 1.642240 | 516.00 | 1.261628 |
