# iperf 结果

- 时间: `0621-141850`
- RV: `/tmp/starryos-rv-recv-cmsg-on-demand.log`
- LA: `/tmp/starryos-la-usercopy-width-tail.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 9.979827 | 6.857305 |
| musl | 9.912219 | 6.802676 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3686.40 | 1.784071 | 849.00 | 1.062426 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 899.00 | 1.562848 | 517.00 | 1.239845 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6021.12 | 1.782313 | 952.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 967.00 | 1.542916 | 529.00 | 1.164461 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3522.56 | 1.775164 | 870.00 | 1.089655 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 815.00 | 1.532515 | 545.00 | 1.300917 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3491.84 | 1.772040 | 835.00 | 1.046707 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 808.00 | 1.513614 | 508.00 | 1.226378 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 5775.36 | 1.773050 | 909.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 933.00 | 1.526259 | 531.00 | 1.167608 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3031.04 | 1.738704 | 847.00 | 1.064935 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 926.00 | 1.588553 | 542.00 | 1.297048 |
