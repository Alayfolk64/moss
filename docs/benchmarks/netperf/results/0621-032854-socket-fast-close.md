# netperf 结果

- 时间: `0621-032854`
- RV: `/tmp/starryos-rv-socket-fast-close.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.777408 | - (缺 5 项) |
| musl | 6.711070 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 160.17 | 1.828307 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 285.62 | 1.720783 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2101.66 | 1.014769 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2565.93 | 1.213548 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 241.58 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 168.92 | 1.837201 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 284.02 | 1.719210 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2074.16 | 1.001707 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2382.37 | 1.152953 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 230.51 | 1.000000 | - | - |
