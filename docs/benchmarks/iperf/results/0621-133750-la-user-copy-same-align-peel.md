# iperf 结果

- 时间: `0621-133806`
- RV: `/tmp/starryos-rv-local-register-role.log`
- LA: `/tmp/starryos-la-user-copy-same-align-peel.log`
- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 10.306997 | 6.893142 |
| musl | 10.293866 | 6.892503 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 4034.56 | 1.802705 | 855.00 | 1.069006 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1054.72 | 1.627389 | 530.00 | 1.258491 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 7055.36 | 1.814224 | 931.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1105.92 | 1.600333 | 536.00 | 1.175373 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4034.56 | 1.803696 | 859.00 | 1.077998 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1116.16 | 1.658651 | 554.00 | 1.312274 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iperf BASIC_TCP (Mbits/sec) | 796.00 | 3962.88 | 1.799136 | 864.00 | 1.078704 |
| iperf BASIC_UDP (Mbits/sec) | 393.00 | 1044.48 | 1.623736 | 523.00 | 1.248566 |
| iperf PARALLEL_TCP (Mbits/sec) | 1310.72 | 6891.52 | 1.809807 | 968.00 | 1.000000 |
| iperf PARALLEL_UDP (Mbits/sec) | 442.00 | 1085.44 | 1.592792 | 539.00 | 1.179963 |
| iperf REVERSE_TCP (Mbits/sec) | 792.00 | 4096.00 | 1.806641 | 859.00 | 1.077998 |
| iperf REVERSE_UDP (Mbits/sec) | 381.00 | 1126.40 | 1.661754 | 550.00 | 1.307273 |
