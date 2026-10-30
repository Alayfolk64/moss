# netperf 结果

- 时间: `0621-031211`
- RV: `/tmp/starryos-rv-udp-no-route-wake.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.798738 | - (缺 5 项) |
| musl | 6.779607 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 186.66 | 1.852673 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 318.31 | 1.749458 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2048.78 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2511.82 | 1.196606 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 238.49 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 183.44 | 1.850087 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 308.19 | 1.741231 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1851.82 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2486.08 | 1.188288 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 242.92 | 1.000000 | - | - |
