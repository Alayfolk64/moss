# netperf 结果

- 时间: `0621-034933`
- RV: `/tmp/starryos-rv-local-register-role.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.286658 | - (缺 5 项) |
| musl | 7.297295 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 171.73 | 1.839865 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 366.81 | 1.782585 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2553.92 | 1.189239 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3843.55 | 1.474970 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 248.21 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.14 | 1.848184 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 364.23 | 1.781045 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2584.81 | 1.198928 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3801.33 | 1.469138 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 244.89 | 1.000000 | - | - |
