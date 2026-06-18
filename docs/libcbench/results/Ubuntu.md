# libcbench 计分结果

- 生成时间: `2026-06-18 02:47:12`
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

- ubuntu_musl: `/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/musl_libcbench.log`
- ubuntu_glibc: `/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/glibc_libcbench.log`

## 汇总

| 输入 | libc | 已解析 | 缺失 | 总分 |
|---|---|---:|---:|---:|
| ubuntu_musl | musl | 27 | 0 | 29.175053 |
| ubuntu_glibc | glibc | 27 | 0 | 33.279239 |

## ubuntu_musl / musl

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| libc-bench b_malloc_sparse (0) (seconds) | 0.384919 | 0.227803 | 1.408180 |
| libc-bench b_malloc_bubble (0) (seconds) | 0.360153 | 0.241742 | 1.328780 |
| libc-bench b_malloc_tiny1 (0) (seconds) | 0.014186 | 0.014032 | 1.010874 |
| libc-bench b_malloc_tiny2 (0) (seconds) | 0.010810 | 0.009348 | 1.135245 |
| libc-bench b_malloc_big1 (0) (seconds) | 0.118578 | 0.105408 | 1.111062 |
| libc-bench b_malloc_big2 (0) (seconds) | 0.107089 | 0.068375 | 1.361507 |
| libc-bench b_malloc_thread_stress (0) (seconds) | 0.096325 | 0.190587 | 1.000000 |
| libc-bench b_malloc_thread_local (0) (seconds) | 0.096990 | 0.129701 | 1.000000 |
| libc-bench b_string_strstr ("abcdefghijklmnopqrstuvwxyz") (seconds) | 0.011402 | 0.079178 | 1.000000 |
| libc-bench b_string_strstr ("azbycxdwevfugthsirjqkplomn") (seconds) | 0.020868 | 0.095988 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaacccccccccccc") (seconds) | 0.011944 | 0.078668 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.011750 | 0.079220 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.013963 | 0.096877 | 1.000000 |
| libc-bench b_string_memset (0) (seconds) | 0.025313 | 0.013354 | 1.472450 |
| libc-bench b_string_strchr (0) (seconds) | 0.012811 | 0.040649 | 1.000000 |
| libc-bench b_string_strlen (0) (seconds) | 0.013459 | 0.038886 | 1.000000 |
| libc-bench b_pthread_createjoin_serial1 (0) (seconds) | 1.284792 | 1.669117 | 1.000000 |
| libc-bench b_pthread_createjoin_serial2 (0) (seconds) | 1.006434 | 1.177649 | 1.000000 |
| libc-bench b_pthread_create_serial1 (0) (seconds) | 0.892421 | 0.763432 | 1.144538 |
| libc-bench b_pthread_uselesslock (0) (seconds) | 0.080551 | 0.184495 | 1.000000 |
| libc-bench b_utf8_bigbuf (0) (seconds) | 0.036978 | 0.203715 | 1.000000 |
| libc-bench b_utf8_onebyone (0) (seconds) | 0.109600 | 0.541761 | 1.000000 |
| libc-bench b_stdio_putcgetc (0) (seconds) | 0.767823 | 0.725145 | 1.055583 |
| libc-bench b_stdio_putcgetc_unlocked (0) (seconds) | 0.765147 | 0.652798 | 1.146833 |
| libc-bench b_regex_compile ("(a|b|c)*d*b") (seconds) | 0.088259 | 0.592019 | 1.000000 |
| libc-bench b_regex_search ("(a|b|c)*d*b") (seconds) | 0.083251 | 0.278074 | 1.000000 |
| libc-bench b_regex_search ("a{25}b") (seconds) | 0.286712 | 0.580701 | 1.000000 |

## ubuntu_glibc / glibc

| 项目 | baseline | result | score |
|---|---:|---:|---:|
| libc-bench b_malloc_sparse (0) (seconds) | 0.384919 | 0.151552 | 1.606277 |
| libc-bench b_malloc_bubble (0) (seconds) | 0.360153 | 0.149636 | 1.584523 |
| libc-bench b_malloc_tiny1 (0) (seconds) | 0.014186 | 0.010226 | 1.279162 |
| libc-bench b_malloc_tiny2 (0) (seconds) | 0.010810 | 0.007338 | 1.321184 |
| libc-bench b_malloc_big1 (0) (seconds) | 0.118578 | 0.043140 | 1.636188 |
| libc-bench b_malloc_big2 (0) (seconds) | 0.107089 | 0.034033 | 1.682197 |
| libc-bench b_malloc_thread_stress (0) (seconds) | 0.096325 | 0.079040 | 1.179446 |
| libc-bench b_malloc_thread_local (0) (seconds) | 0.096990 | 0.059275 | 1.388851 |
| libc-bench b_string_strstr ("abcdefghijklmnopqrstuvwxyz") (seconds) | 0.011402 | 0.041947 | 1.000000 |
| libc-bench b_string_strstr ("azbycxdwevfugthsirjqkplomn") (seconds) | 0.020868 | 0.041926 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaacccccccccccc") (seconds) | 0.011944 | 0.087031 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.011750 | 0.042322 | 1.000000 |
| libc-bench b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac") (seconds) | 0.013963 | 0.047328 | 1.000000 |
| libc-bench b_string_memset (0) (seconds) | 0.025313 | 0.008870 | 1.649591 |
| libc-bench b_string_strchr (0) (seconds) | 0.012811 | 0.040584 | 1.000000 |
| libc-bench b_string_strlen (0) (seconds) | 0.013459 | 0.038683 | 1.000000 |
| libc-bench b_pthread_createjoin_serial1 (0) (seconds) | 1.284792 | 0.853633 | 1.335586 |
| libc-bench b_pthread_createjoin_serial2 (0) (seconds) | 1.006434 | 1.466254 | 1.000000 |
| libc-bench b_pthread_create_serial1 (0) (seconds) | 0.892421 | 0.753058 | 1.156163 |
| libc-bench b_pthread_uselesslock (0) (seconds) | 0.080551 | 0.218282 | 1.000000 |
| libc-bench b_utf8_bigbuf (0) (seconds) | 0.036978 | 0.346267 | 1.000000 |
| libc-bench b_utf8_onebyone (0) (seconds) | 0.109600 | 1.247078 | 1.000000 |
| libc-bench b_stdio_putcgetc (0) (seconds) | 0.767823 | 0.644347 | 1.160813 |
| libc-bench b_stdio_putcgetc_unlocked (0) (seconds) | 0.765147 | 0.553389 | 1.276754 |
| libc-bench b_regex_compile ("(a|b|c)*d*b") (seconds) | 0.088259 | 0.064140 | 1.273275 |
| libc-bench b_regex_search ("(a|b|c)*d*b") (seconds) | 0.083251 | 0.020877 | 1.749228 |
| libc-bench b_regex_search ("a{25}b") (seconds) | 0.286712 | 0.554025 | 1.000000 |
