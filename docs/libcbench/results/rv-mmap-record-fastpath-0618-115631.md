# libcbench 计分结果

- 生成时间: `2026-06-18 11:56:31`
- baseline 来自官方 `judge_libcbench-musl.py` / `judge_libcbench-glibc.py`。
- 计分项: `27`，单个 libc 理论分数范围为 `0` 到 `< 54`。
- 官方 key 形如 `libc-bench <case> (seconds)`，全部按越低越好计分。
- 缺失或非正数结果得 `0`；单项达到 baseline 得 `1.0`，优于 baseline 后逐渐接近 `2.0`。

## 计分规则

```text
result <= baseline: score = 2 - result / baseline
result >  baseline: score = 1
missing/result <= 0: score = 0
```

## 输入

- rv_mmap_record_fastpath: `docs/libcbench/results/rv-mmap-record-fastpath-0618-115631.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv_mmap_record_fastpath | musl | 27 | 0 | 28.097977 |

## rv_mmap_record_fastpath / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| libc-bench b_malloc_sparse (0) (seconds) | 0.384919 | 0.640261 | 1.000000 |
| libc-bench b_malloc_bubble (0) (seconds) | 0.360153 | 0.517656 | 1.000000 |
| libc-bench b_malloc_tiny1 (0) (seconds) | 0.014186 | 0.027774 | 1.000000 |
| libc-bench b_malloc_tiny2 (0) (seconds) | 0.010810 | 0.021106 | 1.000000 |
| libc-bench b_malloc_big1 (0) (seconds) | 0.118578 | 0.201486 | 1.000000 |
| libc-bench b_malloc_big2 (0) (seconds) | 0.107089 | 0.200641 | 1.000000 |
| libc-bench b_malloc_thread_stress (0) (seconds) | 0.096325 | 0.147190 | 1.000000 |
| libc-bench b_malloc_thread_local (0) (seconds) | 0.096990 | 0.116930 | 1.000000 |
| libc-bench b_string_strstr ("abcdefghijklmnopqrstuvwxyz") (seconds) | 0.011402 | 0.032990 | 1.000000 |
| libc-bench b_string_strstr ("azbycxdwevfugthsirjqkplomn") (seconds) | 0.020868 | 0.029120 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaacccccccccccc") (seconds) | 0.011944 | 0.023224 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.011750 | 0.022867 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.013963 | 0.032785 | 1.000000 |
| libc-bench b_string_memset (0) (seconds) | 0.025313 | 0.016851 | 1.334284 |
| libc-bench b_string_strchr (0) (seconds) | 0.012811 | 0.022241 | 1.000000 |
| libc-bench b_string_strlen (0) (seconds) | 0.013459 | 0.020682 | 1.000000 |
| libc-bench b_pthread_createjoin_serial1 (0) (seconds) | 1.284792 | 1.137199 | 1.114876 |
| libc-bench b_pthread_createjoin_serial2 (0) (seconds) | 1.006434 | 0.906041 | 1.099751 |
| libc-bench b_pthread_create_serial1 (0) (seconds) | 0.892421 | 1.178997 | 1.000000 |
| libc-bench b_pthread_uselesslock (0) (seconds) | 0.080551 | 0.094085 | 1.000000 |
| libc-bench b_utf8_bigbuf (0) (seconds) | 0.036978 | 0.064939 | 1.000000 |
| libc-bench b_utf8_onebyone (0) (seconds) | 0.109600 | 0.171950 | 1.000000 |
| libc-bench b_stdio_putcgetc (0) (seconds) | 0.767823 | 0.578219 | 1.246937 |
| libc-bench b_stdio_putcgetc_unlocked (0) (seconds) | 0.765147 | 0.533974 | 1.302129 |
| libc-bench b_regex_compile ("(a|b|c)*d*b") (seconds) | 0.088259 | 0.142539 | 1.000000 |
| libc-bench b_regex_search ("(a|b|c)*d*b") (seconds) | 0.083251 | 0.158350 | 1.000000 |
| libc-bench b_regex_search ("a{25}b") (seconds) | 0.286712 | 0.605837 | 1.000000 |
