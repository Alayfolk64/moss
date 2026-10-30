# netperf 结果

- 时间: `0621-035909`
- RV: `/tmp/starryos-rv-socket-close-table-fast.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.213068 | - (缺 5 项) |
| musl | 7.232578 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 183.30 | 1.849973 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 373.78 | 1.786639 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2457.40 | 1.157394 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3473.66 | 1.419062 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 243.44 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.49 | 1.848477 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 348.65 | 1.771261 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2406.98 | 1.139744 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3829.89 | 1.473097 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 252.99 | 1.000000 | - | - |
