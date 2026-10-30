# netperf 结果

- 时间: `0621-140053`
- RV: `/tmp/starryos-rv-local-register-role.log`
- LA: `/tmp/starryos-la-udp-inline-payload.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.286658 | 6.592299 |
| musl | 7.297295 | 6.652041 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 171.73 | 1.839865 | 199.35 | 1.862052 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 366.81 | 1.782585 | 210.82 | 1.621715 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2553.92 | 1.189239 | 1556.89 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3843.55 | 1.474970 | 2263.66 | 1.108532 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 248.21 | 1.000000 | 225.07 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.14 | 1.848184 | 170.51 | 1.838719 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 364.23 | 1.781045 | 261.95 | 1.695553 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2584.81 | 1.198928 | 1725.81 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3801.33 | 1.469138 | 2287.36 | 1.117769 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 244.89 | 1.000000 | 232.84 | 1.000000 |
