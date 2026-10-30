# netperf 结果

- 时间: `0621-124117`
- RV: `/tmp/no_rv_netperf.log`
- LA: `/tmp/starryos-la-user-copy-ldbu.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 5 项) | 6.844390 |
| musl | - (缺 5 项) | 6.816635 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 239.72 | 1.885283 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 295.77 | 1.730365 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1843.85 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2616.48 | 1.228742 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 241.58 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 205.99 | 1.866498 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 282.12 | 1.717319 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1881.72 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2630.38 | 1.232818 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 231.61 | 1.000000 |
