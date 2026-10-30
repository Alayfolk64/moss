# netperf 结果

- 时间: `0621-124747`
- RV: `/tmp/no_rv_netperf.log`
- LA: `/tmp/starryos-la-udp-recv-front-fastpath.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 5 项) | 6.567258 |
| musl | - (缺 5 项) | 6.804637 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 192.67 | 1.857269 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 259.54 | 1.692726 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1500.90 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2053.43 | 1.017264 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 202.44 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 227.47 | 1.879105 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 295.32 | 1.729954 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1824.00 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2508.61 | 1.195578 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 242.44 | 1.000000 |
