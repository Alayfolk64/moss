# netperf 结果

- 时间: `0621-031508`
- RV: `/tmp/starryos-rv-udp-no-route-wake.log`
- LA: `/tmp/starryos-la-udp-no-route-wake.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.798738 | 6.562140 |
| musl | 6.779607 | 6.553975 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 186.66 | 1.852673 | 228.46 | 1.879629 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 318.31 | 1.749458 | 251.19 | 1.682511 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2048.78 | 1.000000 | 1360.34 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2511.82 | 1.196606 | 1818.16 | 1.000000 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 238.49 | 1.000000 | 233.75 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 183.44 | 1.850087 | 222.95 | 1.876654 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 308.19 | 1.741231 | 247.15 | 1.677321 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1851.82 | 1.000000 | 1545.77 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2486.08 | 1.188288 | 1802.04 | 1.000000 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 242.92 | 1.000000 | 230.14 | 1.000000 |
