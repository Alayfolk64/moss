# netperf 结果

- 时间: `0621-121112`
- RV: `/tmp/no_rv_netperf.log`
- LA: `/tmp/starryos-la-user-copy-word.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 5 项) | 6.487393 |
| musl | - (缺 5 项) | 6.459771 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 164.00 | 1.832317 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 229.81 | 1.652974 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1396.30 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2022.23 | 1.002102 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 203.76 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 164.94 | 1.833273 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 213.52 | 1.626499 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1376.38 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 1869.64 | 1.000000 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 175.42 | 1.000000 |
