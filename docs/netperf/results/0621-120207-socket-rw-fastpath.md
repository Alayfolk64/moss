# netperf 结果

- 时间: `0621-120207`
- RV: `/tmp/starryos-rv-socket-rw-fastpath.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.456579 | - (缺 5 项) |
| musl | 7.218283 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 121.70 | 1.774035 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 195.24 | 1.591528 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 1013.54 | 1.000000 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2220.04 | 1.091016 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 152.93 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 168.54 | 1.836834 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 353.68 | 1.774514 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2524.16 | 1.179680 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3523.35 | 1.427255 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 242.35 | 1.000000 | - | - |
