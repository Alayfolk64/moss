# netperf 结果

- 时间: `0621-135341`
- RV: `/tmp/starryos-rv-recv-cmsg-on-demand.log`
- LA: `/tmp/starryos-la-recv-cmsg-on-demand.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.955725 | 6.844982 |
| musl | 7.097010 | 6.844623 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 165.05 | 1.833384 | 237.02 | 1.883976 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 281.24 | 1.716434 | 290.06 | 1.725057 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2210.31 | 1.063199 | 1853.85 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3070.14 | 1.342707 | 2641.16 | 1.235949 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 197.56 | 1.000000 | 249.87 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 163.33 | 1.831629 | 241.34 | 1.886053 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 319.72 | 1.750563 | 296.14 | 1.730702 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2441.27 | 1.151827 | 1928.46 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3167.90 | 1.362991 | 2613.52 | 1.227869 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 230.17 | 1.000000 | 244.66 | 1.000000 |
