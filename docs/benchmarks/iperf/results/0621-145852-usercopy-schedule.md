# iperf 结果

- 时间: `0621-145905`
- RV: `/tmp/starryos-rv-recv-cmsg-on-demand.log`
- LA: `/tmp/starryos-la-usercopy-schedule.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 9.979827 | 6.183203 |
| musl | 9.912219 | 6.585925 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3686.40 | 1.784071 | 708.00 | 1.000000 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 899.00 | 1.562848 | 430.00 | 1.086047 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6021.12 | 1.782313 | 760.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 967.00 | 1.542916 | 427.00 | 1.000000 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3522.56 | 1.775164 | 686.00 | 1.000000 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 815.00 | 1.532515 | 422.00 | 1.097156 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3491.84 | 1.772040 | 812.00 | 1.019704 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 808.00 | 1.513614 | 487.00 | 1.193018 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 5775.36 | 1.773050 | 825.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 933.00 | 1.526259 | 492.00 | 1.101626 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 3031.04 | 1.738704 | 840.00 | 1.057143 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 926.00 | 1.588553 | 485.00 | 1.214433 |
