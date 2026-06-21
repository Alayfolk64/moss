# iperf 结果

- 时间: `0621-120207`
- RV: `/tmp/starryos-rv-socket-rw-fastpath.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 8.582051 | - (缺 6 项) |
| musl | 9.643959 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 2600.96 | 1.693959 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 412.00 | 1.046117 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 5253.12 | 1.750487 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 713.00 | 1.380084 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 2744.32 | 1.711404 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 316.00 | 1.000000 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3420.16 | 1.767262 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 458.00 | 1.141921 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 5416.96 | 1.758034 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1054.72 | 1.580931 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3358.72 | 1.764196 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1034.24 | 1.631614 | - | - |
