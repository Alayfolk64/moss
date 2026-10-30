# netperf 结果

- 时间: `0621-031912`
- RV: `docs/output_rv.txt`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.800748 | - (缺 5 项) |
| musl | 6.735130 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 179.02 | 1.846386 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 309.95 | 1.742700 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2018.55 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2559.79 | 1.211662 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 239.28 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 162.04 | 1.830289 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 285.54 | 1.720705 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2102.88 | 1.015341 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2427.78 | 1.168796 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 241.64 | 1.000000 | - | - |
