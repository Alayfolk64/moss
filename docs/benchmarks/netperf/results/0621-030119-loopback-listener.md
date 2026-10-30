# netperf 结果

- 时间: `0621-030143`
- RV: `/tmp/starryos-rv-loopback-listener.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.782809 | - (缺 5 项) |
| musl | 6.762586 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 177.77 | 1.845306 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 315.94 | 1.747579 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1829.80 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2491.10 | 1.189924 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 232.38 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 174.59 | 1.842488 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 313.39 | 1.745525 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1822.41 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2444.77 | 1.174573 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 240.41 | 1.000000 | - | - |
