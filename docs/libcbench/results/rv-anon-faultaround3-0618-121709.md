# libcbench 计分结果

- 生成时间: `2026-06-18 12:17:09`
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

- rv_anon_faultaround3: `docs/libcbench/results/rv-anon-faultaround3-0618-121709.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv_anon_faultaround3 | musl | 27 | 0 | 28.164660 |

## rv_anon_faultaround3 / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| libc-bench b_malloc_sparse (0) (seconds) | 0.384919 | 0.611474 | 1.000000 |
| libc-bench b_malloc_bubble (0) (seconds) | 0.360153 | 0.542763 | 1.000000 |
| libc-bench b_malloc_tiny1 (0) (seconds) | 0.014186 | 0.026650 | 1.000000 |
| libc-bench b_malloc_tiny2 (0) (seconds) | 0.010810 | 0.020911 | 1.000000 |
| libc-bench b_malloc_big1 (0) (seconds) | 0.118578 | 0.225891 | 1.000000 |
| libc-bench b_malloc_big2 (0) (seconds) | 0.107089 | 0.221308 | 1.000000 |
| libc-bench b_malloc_thread_stress (0) (seconds) | 0.096325 | 0.156945 | 1.000000 |
| libc-bench b_malloc_thread_local (0) (seconds) | 0.096990 | 0.118607 | 1.000000 |
| libc-bench b_string_strstr ("abcdefghijklmnopqrstuvwxyz") (seconds) | 0.011402 | 0.036035 | 1.000000 |
| libc-bench b_string_strstr ("azbycxdwevfugthsirjqkplomn") (seconds) | 0.020868 | 0.032665 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaacccccccccccc") (seconds) | 0.011944 | 0.024007 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.011750 | 0.023602 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.013963 | 0.030549 | 1.000000 |
| libc-bench b_string_memset (0) (seconds) | 0.025313 | 0.016361 | 1.353654 |
| libc-bench b_string_strchr (0) (seconds) | 0.012811 | 0.021987 | 1.000000 |
| libc-bench b_string_strlen (0) (seconds) | 0.013459 | 0.020797 | 1.000000 |
| libc-bench b_pthread_createjoin_serial1 (0) (seconds) | 1.284792 | 1.127944 | 1.122080 |
| libc-bench b_pthread_createjoin_serial2 (0) (seconds) | 1.006434 | 0.858762 | 1.146728 |
| libc-bench b_pthread_create_serial1 (0) (seconds) | 0.892421 | 1.325826 | 1.000000 |
| libc-bench b_pthread_uselesslock (0) (seconds) | 0.080551 | 0.097715 | 1.000000 |
| libc-bench b_utf8_bigbuf (0) (seconds) | 0.036978 | 0.060009 | 1.000000 |
| libc-bench b_utf8_onebyone (0) (seconds) | 0.109600 | 0.187531 | 1.000000 |
| libc-bench b_stdio_putcgetc (0) (seconds) | 0.767823 | 0.575888 | 1.249973 |
| libc-bench b_stdio_putcgetc_unlocked (0) (seconds) | 0.765147 | 0.541551 | 1.292226 |
| libc-bench b_regex_compile ("(a|b|c)*d*b") (seconds) | 0.088259 | 0.144469 | 1.000000 |
| libc-bench b_regex_search ("(a|b|c)*d*b") (seconds) | 0.083251 | 0.169416 | 1.000000 |
| libc-bench b_regex_search ("a{25}b") (seconds) | 0.286712 | 0.651629 | 1.000000 |
