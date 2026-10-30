# netperf 结果

- 时间: `0621-121629`
- RV: `/tmp/no_rv_netperf.log`
- LA: `/tmp/starryos-la-ring-write-two-slices.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 5 项) | 6.645026 |
| musl | - (缺 5 项) | 6.608227 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 195.68 | 1.859464 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 260.25 | 1.693564 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1603.76 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2222.44 | 1.091998 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 204.29 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 167.33 | 1.835654 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 240.05 | 1.667778 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1624.37 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2254.21 | 1.104795 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 208.33 | 1.000000 |
