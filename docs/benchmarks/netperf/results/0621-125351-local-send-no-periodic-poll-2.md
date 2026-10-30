# netperf 结果

- 时间: `0621-125352`
- RV: `/tmp/no_rv_netperf.log`
- LA: `/tmp/starryos-la-local-send-no-periodic-poll-2.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 5 项) | 6.809421 |
| musl | - (缺 5 项) | 6.801632 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 234.57 | 1.882764 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 295.45 | 1.730073 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1724.56 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2511.75 | 1.196584 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 227.61 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 210.63 | 1.869439 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 288.25 | 1.723330 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1822.00 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2550.73 | 1.208862 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 228.85 | 1.000000 |
