# netperf 结果

- 时间: `0621-123646`
- RV: `/tmp/no_rv_netperf.log`
- LA: `/tmp/starryos-la-user-copy-aligned-dword.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 5 项) | 6.375675 |
| musl | - (缺 5 项) | 6.251331 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 160.15 | 1.828286 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 176.20 | 1.547389 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1015.96 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 1706.50 | 1.000000 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 163.41 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 126.52 | 1.782643 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 150.10 | 1.468688 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1157.70 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 1535.26 | 1.000000 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 152.72 | 1.000000 |
