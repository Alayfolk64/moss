# netperf 结果

- 时间: `0621-122617`
- RV: `/tmp/starryos-rv-local-skip-network-poller.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.727608 | - (缺 5 项) |
| musl | 5.941402 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 161.64 | 1.829869 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 201.85 | 1.604905 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2156.23 | 1.039704 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2701.92 | 1.253131 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 165.66 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 71.16 | 1.613547 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 118.65 | 1.327855 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1261.73 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 1808.42 | 1.000000 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 132.23 | 1.000000 | - | - |
