# libcbench 计分结果

- 生成时间: `2026-06-18 02:54:30`
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

- rv_anon_faultaround2: `docs/libcbench/results/rv-anon-faultaround2-0618-025430.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| rv_anon_faultaround2 | musl | 27 | 0 | 28.181592 |

## rv_anon_faultaround2 / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| libc-bench b_malloc_sparse (0) (seconds) | 0.384919 | 0.604008 | 1.000000 |
| libc-bench b_malloc_bubble (0) (seconds) | 0.360153 | 0.519493 | 1.000000 |
| libc-bench b_malloc_tiny1 (0) (seconds) | 0.014186 | 0.027735 | 1.000000 |
| libc-bench b_malloc_tiny2 (0) (seconds) | 0.010810 | 0.021283 | 1.000000 |
| libc-bench b_malloc_big1 (0) (seconds) | 0.118578 | 0.201976 | 1.000000 |
| libc-bench b_malloc_big2 (0) (seconds) | 0.107089 | 0.197904 | 1.000000 |
| libc-bench b_malloc_thread_stress (0) (seconds) | 0.096325 | 0.150882 | 1.000000 |
| libc-bench b_malloc_thread_local (0) (seconds) | 0.096990 | 0.119573 | 1.000000 |
| libc-bench b_string_strstr ("abcdefghijklmnopqrstuvwxyz") (seconds) | 0.011402 | 0.033232 | 1.000000 |
| libc-bench b_string_strstr ("azbycxdwevfugthsirjqkplomn") (seconds) | 0.020868 | 0.029762 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaacccccccccccc") (seconds) | 0.011944 | 0.023731 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.011750 | 0.023118 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.013963 | 0.033195 | 1.000000 |
| libc-bench b_string_memset (0) (seconds) | 0.025313 | 0.016233 | 1.358718 |
| libc-bench b_string_strchr (0) (seconds) | 0.012811 | 0.024499 | 1.000000 |
| libc-bench b_string_strlen (0) (seconds) | 0.013459 | 0.020990 | 1.000000 |
| libc-bench b_pthread_createjoin_serial1 (0) (seconds) | 1.284792 | 1.132512 | 1.118524 |
| libc-bench b_pthread_createjoin_serial2 (0) (seconds) | 1.006434 | 0.843374 | 1.162018 |
| libc-bench b_pthread_create_serial1 (0) (seconds) | 0.892421 | 1.339177 | 1.000000 |
| libc-bench b_pthread_uselesslock (0) (seconds) | 0.080551 | 0.096377 | 1.000000 |
| libc-bench b_utf8_bigbuf (0) (seconds) | 0.036978 | 0.060838 | 1.000000 |
| libc-bench b_utf8_onebyone (0) (seconds) | 0.109600 | 0.188764 | 1.000000 |
| libc-bench b_stdio_putcgetc (0) (seconds) | 0.767823 | 0.585818 | 1.237040 |
| libc-bench b_stdio_putcgetc_unlocked (0) (seconds) | 0.765147 | 0.531555 | 1.305291 |
| libc-bench b_regex_compile ("(a|b|c)*d*b") (seconds) | 0.088259 | 0.142402 | 1.000000 |
| libc-bench b_regex_search ("(a|b|c)*d*b") (seconds) | 0.083251 | 0.163961 | 1.000000 |
| libc-bench b_regex_search ("a{25}b") (seconds) | 0.286712 | 0.602594 | 1.000000 |
