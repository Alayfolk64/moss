# netperf 结果

- 时间: `0621-114826`
- RV: `/tmp/starryos-rv-lookup-tcp-direct.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.287864 | - (缺 5 项) |
| musl | 7.076763 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.70 | 1.848652 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 360.39 | 1.778712 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2541.24 | 1.185193 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3846.02 | 1.475307 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 257.80 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 163.60 | 1.831907 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 329.35 | 1.757856 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2198.61 | 1.058214 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3532.79 | 1.428786 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 242.31 | 1.000000 | - | - |
