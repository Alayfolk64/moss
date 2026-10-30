# netperf 结果

- 时间: `0621-035221`
- RV: `/tmp/starryos-rv-local-register-role.log`
- LA: `/tmp/starryos-la-local-register-role.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.286658 | 6.826509 |
| musl | 7.297295 | 6.823195 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 171.73 | 1.839865 | 234.69 | 1.882824 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 366.81 | 1.782585 | 303.52 | 1.737250 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2553.92 | 1.189239 | 1898.61 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3843.55 | 1.474970 | 2542.93 | 1.206435 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 248.21 | 1.000000 | 239.09 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.14 | 1.848184 | 219.74 | 1.874852 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 364.23 | 1.781045 | 283.73 | 1.718923 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2584.81 | 1.198928 | 1803.73 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3801.33 | 1.469138 | 2618.78 | 1.229420 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 244.89 | 1.000000 | 239.09 | 1.000000 |
