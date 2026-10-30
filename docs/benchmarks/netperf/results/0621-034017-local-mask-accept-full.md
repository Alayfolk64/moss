# netperf 结果

- 时间: `0621-034017`
- RV: `/tmp/starryos-rv-local-mask-accept.log`
- LA: `/tmp/starryos-la-local-mask-accept.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.216496 | 6.777446 |
| musl | 7.195117 | 6.676504 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 183.16 | 1.849858 | 232.14 | 1.881537 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 351.08 | 1.772844 | 290.73 | 1.725691 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2578.30 | 1.196905 | 1796.03 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3345.95 | 1.396889 | 2431.94 | 1.170218 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 244.30 | 1.000000 | 236.72 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 164.79 | 1.833121 | 219.77 | 1.874869 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 345.43 | 1.769128 | 271.67 | 1.706445 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2567.59 | 1.193555 | 1817.78 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3359.45 | 1.399312 | 2230.28 | 1.095190 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 242.92 | 1.000000 | 237.87 | 1.000000 |
