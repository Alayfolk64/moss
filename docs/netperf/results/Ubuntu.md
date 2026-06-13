# netperf 结果

- 时间: `0613-005714`
- RV: `/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/glibc_netperf.log`, `/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/musl_netperf.log`
- LA: `/tmp/no_la_netperf.log`
- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 7.447960 | - (缺 5 项) |
| musl | 8.089466 | - (缺 5 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 127.63 | 1.784533 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 277.37 | 1.712478 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 2807.66 | 1.262510 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 2544.46 | 1.206912 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 753.23 | 1.481526 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| netperf UDP_STREAM (Trans. Rate/sec) | 27.50 | 181.98 | 1.848884 | - | - |
| netperf TCP_STREAM (Throughput 10^6bits/sec) | 79.75 | 448.45 | 1.822165 | - | - |
| netperf UDP_RR (Trans. Rate/sec) | 2070.62 | 3682.67 | 1.437739 | - | - |
| netperf TCP_RR (Trans. Rate/sec) | 2017.98 | 3300.20 | 1.388528 | - | - |
| netperf TCP_CRR (Trans. Rate/sec) | 390.53 | 957.53 | 1.592149 | - | - |
