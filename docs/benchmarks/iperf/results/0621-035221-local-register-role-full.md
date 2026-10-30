# iperf 结果

- 时间: `0621-035221`
- RV: `/tmp/starryos-rv-local-register-role.log`
- LA: `/tmp/starryos-la-local-register-role.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.306997 | 6.678190 |
| musl | 10.293866 | 6.742085 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 4034.56 | 1.802705 | 807.00 | 1.013631 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1054.72 | 1.627389 | 506.00 | 1.223320 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 7055.36 | 1.814224 | 908.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1105.92 | 1.600333 | 509.00 | 1.131631 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4034.56 | 1.803696 | 821.00 | 1.035323 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1116.16 | 1.658651 | 525.00 | 1.274286 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3962.88 | 1.799136 | 837.00 | 1.048984 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1044.48 | 1.623736 | 500.00 | 1.214000 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6891.52 | 1.809807 | 902.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1085.44 | 1.592792 | 510.00 | 1.133333 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4096.00 | 1.806641 | 836.00 | 1.052632 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1126.40 | 1.661754 | 539.00 | 1.293135 |
