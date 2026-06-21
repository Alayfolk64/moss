# netperf 结果

- 时间: `0621-145905`
- RV: `/tmp/starryos-rv-recv-cmsg-on-demand.log`
- LA: `/tmp/starryos-la-usercopy-schedule.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 6.955725 | 6.611769 |
| musl | 7.097010 | 6.781488 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 165.05 | 1.833384 | 189.55 | 1.854920 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 281.24 | 1.716434 | 264.22 | 1.698168 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2210.31 | 1.063199 | 1181.66 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3070.14 | 1.342707 | 2143.78 | 1.058681 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 197.56 | 1.000000 | 216.65 | 1.000000 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 163.33 | 1.831629 | 231.76 | 1.881343 |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 319.72 | 1.750563 | 261.53 | 1.695064 |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2441.27 | 1.151827 | 1839.45 | 1.000000 |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3167.90 | 1.362991 | 2538.60 | 1.205082 |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 230.17 | 1.000000 | 230.25 | 1.000000 |
