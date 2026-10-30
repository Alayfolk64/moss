# iperf 结果

- 时间: `0621-030521`
- RV: `/tmp/starryos-rv-loopback-listener.log`
- LA: `/tmp/starryos-la-loopback-listener.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.193185 | 6.738831 |
| musl | 10.173600 | 6.759961 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3635.20 | 1.781030 | 817.00 | 1.025704 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 991.00 | 1.603431 | 504.00 | 1.220238 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6195.20 | 1.788430 | 916.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1064.96 | 1.584961 | 511.00 | 1.135029 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3706.88 | 1.786343 | 848.00 | 1.066038 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1085.44 | 1.648990 | 538.00 | 1.291822 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3624.96 | 1.780411 | 831.00 | 1.042118 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1024.00 | 1.616211 | 515.00 | 1.236893 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6092.80 | 1.784874 | 912.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 992.00 | 1.554435 | 512.00 | 1.136719 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3747.84 | 1.788678 | 858.00 | 1.076923 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1085.44 | 1.648990 | 520.00 | 1.267308 |
