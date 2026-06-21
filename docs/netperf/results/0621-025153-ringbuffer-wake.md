# netperf 结果

- 时间: `0621-025330`
- RV: `/tmp/starryos-rv-ringbuffer-wake.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.788280 | - (缺 5 项) |
| musl | 6.699279 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 174.89 | 1.842758 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 321.11 | 1.751643 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1843.15 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2503.32 | 1.193879 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 234.53 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 161.53 | 1.829753 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 277.92 | 1.713047 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1627.14 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2392.33 | 1.156479 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 221.09 | 1.000000 | - | - |
