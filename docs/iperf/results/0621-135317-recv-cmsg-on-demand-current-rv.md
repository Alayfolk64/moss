# iperf 结果

- 时间: `0621-135341`
- RV: `/tmp/starryos-rv-recv-cmsg-on-demand.log`
- LA: `/tmp/starryos-la-recv-cmsg-on-demand.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 9.979827 | 6.962235 |
| musl | 9.912219 | 6.945745 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3686.40 | 1.784071 | 866.00 | 1.080831 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 899.00 | 1.562848 | 538.00 | 1.269517 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6021.12 | 1.782313 | 973.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 967.00 | 1.542916 | 545.00 | 1.188991 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3522.56 | 1.775164 | 882.00 | 1.102041 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 815.00 | 1.532515 | 561.00 | 1.320856 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3491.84 | 1.772040 | 884.00 | 1.099548 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 808.00 | 1.513614 | 533.00 | 1.262664 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 5775.36 | 1.773050 | 987.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 933.00 | 1.526259 | 541.00 | 1.182994 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3031.04 | 1.738704 | 855.00 | 1.073684 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 926.00 | 1.588553 | 566.00 | 1.326855 |
