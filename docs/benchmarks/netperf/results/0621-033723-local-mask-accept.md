# netperf 结果

- 时间: `0621-033723`
- RV: `/tmp/starryos-rv-local-mask-accept.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.216496 | - (缺 5 项) |
| musl | 7.195117 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 183.16 | 1.849858 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 351.08 | 1.772844 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2578.30 | 1.196905 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3345.95 | 1.396889 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 244.30 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 164.79 | 1.833121 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 345.43 | 1.769128 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2567.59 | 1.193555 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3359.45 | 1.399312 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 242.92 | 1.000000 | - | - |
