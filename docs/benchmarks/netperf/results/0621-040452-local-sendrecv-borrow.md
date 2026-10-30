# netperf 结果

- 时间: `0621-040453`
- RV: `/tmp/starryos-rv-local-sendrecv-borrow.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.269724 | - (缺 5 项) |
| musl | 7.268835 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 191.53 | 1.856419 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 373.06 | 1.786227 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2496.69 | 1.170654 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3712.41 | 1.456423 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 245.36 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 177.38 | 1.844966 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 360.92 | 1.779037 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2533.82 | 1.182807 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3751.07 | 1.462026 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 249.04 | 1.000000 | - | - |
