# iperf 结果

- 时间: `0612-020712`
- RV: `/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/glibc_iperf.log`, `/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/musl_iperf.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.740733 | - (缺 6 项) |
| musl | 10.604013 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3041.28 | 1.738268 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 2908.16 | 1.864863 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 3573.76 | 1.633238 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 3840.00 | 1.884896 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3276.80 | 1.758301 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 2744.32 | 1.861168 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3266.56 | 1.756319 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 2324.48 | 1.830930 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 2754.56 | 1.524164 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 3809.28 | 1.883968 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3379.20 | 1.765625 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 2426.88 | 1.843008 | - | - |
