# netperf 结果

- 时间: `0621-125228`
- RV: `/tmp/no_rv_netperf.log`
- LA: `/tmp/starryos-la-local-send-no-periodic-poll.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | - (缺 5 项) | 6.478934 |
| musl | - (缺 5 项) | 6.787650 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 171.01 | 1.839191 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 221.37 | 1.639743 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1387.54 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 1867.88 | 1.000000 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 139.99 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | - | - | 220.75 | 1.875425 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | - | - | 287.03 | 1.722154 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | - | - | 1874.76 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | - | - | 2491.55 | 1.190070 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | - | - | 244.00 | 1.000000 |
