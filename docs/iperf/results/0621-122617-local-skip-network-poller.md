# iperf 结果

- 时间: `0621-122617`
- RV: `/tmp/starryos-rv-local-skip-network-poller.log`
- LA: `/tmp/no_la_iperf.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 9.712737 | - (缺 6 项) |
| musl | 9.766339 | - (缺 6 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3399.68 | 1.765860 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 828.00 | 1.525362 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 4505.60 | 1.709091 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 870.00 | 1.491954 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 2938.88 | 1.730510 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 747.00 | 1.489960 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3266.56 | 1.756319 | - | - |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 832.00 | 1.527644 | - | - |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 4925.44 | 1.733888 | - | - |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 900.00 | 1.508889 | - | - |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3225.60 | 1.754464 | - | - |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 740.00 | 1.485135 | - | - |
