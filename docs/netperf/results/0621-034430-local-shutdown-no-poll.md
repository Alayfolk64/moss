# netperf 结果

- 时间: `0621-034430`
- RV: `/tmp/starryos-rv-local-shutdown-no-poll.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.116038 | - (缺 5 项) |
| musl | 7.210055 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.14 | 1.848184 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 359.47 | 1.778146 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2430.03 | 1.147904 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3065.93 | 1.341805 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 246.41 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 178.41 | 1.845861 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 332.96 | 1.760482 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2583.43 | 1.198500 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3392.78 | 1.405213 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 245.26 | 1.000000 | - | - |
