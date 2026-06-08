# Test Timing Report

统计口径：普通测试组使用组前后时间戳直接做差；LTP 使用内部每个 `RUN LTP CASE` 后打印的时间戳逐项做差并累加，只汇总到 `ltp-*` 组粒度，不展开单个 case。

## LTP Summary

| File | Group | Cases | Valid cases | Clock-jump cases | Incomplete cases | Valid sum | Seconds |
|---|---|---:|---:|---:|---:|---:|---:|
| output_rv.txt | ltp-musl | 161 | 160 | 0 | 1 | 00:05:57 | 357 |
| output_la.txt | ltp-musl | 859 | 858 | 1 | 0 | 00:34:20 | 2060 |
| output_la.txt | ltp-glibc | 859 | 858 | 1 | 0 | 00:46:29 | 2789 |

## output_rv.txt

| Group | Duration | Seconds | Note |
|---|---:|---:|---|
| lmbench-musl | 00:23:55 | 1435 | |
| ltp-musl | 00:05:57 | 357 | LTP case sum; 1 incomplete |
| cyclictest-musl | 00:05:06 | 306 | |
| iozone-musl | 00:02:44 | 164 | |
| libctest-musl | 00:00:38 | 38 | |
| libcbench-musl | 00:00:20 | 20 | |
| netperf-musl | 00:00:11 | 11 | |
| busybox-musl | 00:00:08 | 8 | |
| lua-musl | 00:00:06 | 6 | |
| basic-musl | 00:00:05 | 5 | |
| iperf-musl | 00:00:02 | 2 | |

## output_la.txt

| Group | Duration | Seconds | Note |
|---|---:|---:|---|
| ltp-glibc | 00:46:29 | 2789 | LTP case sum; 1 clock-jump excluded |
| ltp-musl | 00:34:20 | 2060 | LTP case sum; 1 clock-jump excluded |
| lmbench-musl | 00:23:13 | 1393 | |
| lmbench-glibc | 00:22:51 | 1371 | |
| cyclictest-glibc | 00:02:35 | 155 | |
| iozone-musl | 00:02:24 | 144 | |
| cyclictest-musl | 00:02:15 | 135 | |
| iozone-glibc | 00:02:11 | 131 | |
| libctest-musl | 00:00:32 | 32 | |
| netperf-glibc | 00:00:11 | 11 | |
| libcbench-musl | 00:00:10 | 10 | |
| busybox-glibc | 00:00:10 | 10 | |
| netperf-musl | 00:00:10 | 10 | |
| busybox-musl | 00:00:07 | 7 | |
| libcbench-glibc | 00:00:07 | 7 | |
| lua-musl | 00:00:06 | 6 | |
| lua-glibc | 00:00:05 | 5 | |
| basic-musl | 00:00:04 | 4 | |
| basic-glibc | 00:00:04 | 4 | |
| iperf-musl | 00:00:02 | 2 | |
| iperf-glibc | 00:00:02 | 2 | |

