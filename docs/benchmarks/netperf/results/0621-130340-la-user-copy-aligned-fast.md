# netperf 结果

- 时间: `0621-130447`
- RV: `/tmp/starryos-rv-local-register-role.log`
- LA: `/tmp/starryos-la-user-copy-aligned-fast.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.286658 | 6.887820 |
| musl | 7.297295 | 6.849435 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 171.73 | 1.839865 | 237.06 | 1.883996 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 366.81 | 1.782585 | 305.11 | 1.738619 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2553.92 | 1.189239 | 1913.70 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3843.55 | 1.474970 | 2746.32 | 1.265206 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 248.21 | 1.000000 | 246.48 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.14 | 1.848184 | 234.31 | 1.882634 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 364.23 | 1.781045 | 294.36 | 1.729073 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2584.81 | 1.198928 | 1972.24 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3801.33 | 1.469138 | 2647.32 | 1.237727 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 244.89 | 1.000000 | 243.68 | 1.000000 |
