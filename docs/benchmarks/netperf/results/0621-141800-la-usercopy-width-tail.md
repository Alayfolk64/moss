# netperf 结果

- 时间: `0621-141850`
- RV: `/tmp/starryos-rv-recv-cmsg-on-demand.log`
- LA: `/tmp/starryos-la-usercopy-width-tail.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.955725 | 6.852188 |
| musl | 7.097010 | 6.835223 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 165.05 | 1.833384 | 231.32 | 1.881117 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 281.24 | 1.716434 | 292.46 | 1.727313 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2210.31 | 1.063199 | 1864.29 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3070.14 | 1.342707 | 2668.43 | 1.243758 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 197.56 | 1.000000 | 241.36 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 163.33 | 1.831629 | 223.80 | 1.877122 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 319.72 | 1.750563 | 291.75 | 1.726650 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2441.27 | 1.151827 | 1913.15 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3167.90 | 1.362991 | 2625.70 | 1.231451 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 230.17 | 1.000000 | 239.43 | 1.000000 |
