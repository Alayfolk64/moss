# iozone cache retention 优化报告

时间: 2026-06-24

## 背景

对比历史高分和 Ubuntu/Linux 的 page cache 行为，当前实现的主要问题是最后一个 `CachedFile` 关闭时会把该文件的缓存页全部写回并丢弃。这和 Linux 的行为不一致：Linux 上 `close()` 不等于 `drop_caches`，干净页通常会继续留在 page cache 中，后续重新打开同一文件仍可命中缓存。

iozone 的 `readers`、`re-readers`、`pread readers`、`random readers` 等子项会反复创建、关闭、再读取小文件。关闭时丢 clean cache 会让这些子项退回到底层文件系统读取路径，分数明显低于历史高分。

## 修改

文件: `vendor/axfs/src/highlevel/file.rs`

- `CachedFile::drop()` 不再在最后一个 fd 关闭时直接 `drop_cached_pages()`。
- 最后一个 fd 关闭时仍会写回 dirty 页并提交 atime，保证数据不会丢。
- clean 页和缓存记录会保留，供后续 reopen/read 命中。
- 新增 `MAX_RETAINED_FILE_CACHE_RECORDS = 64`，超过上限时清理 inactive 文件缓存，避免无界占用内存。
- 显式失效路径保持不变：`invalidate_file_cache()`、`invalidate_removed_file_cache()`、`drop_file_caches_by_device()`、direct write 后的 `invalidate_clean_pages_in_range()`、truncate/direct set_len 的 `drop_cached_pages()` 仍会清理缓存。

## 结果

评分脚本: `docs/benchmarks/iozone/get_result.py`

| 日志 | RV glibc | RV musl | RV 合计 | LA glibc | LA musl | LA 合计 |
|---|---:|---:|---:|---:|---:|---:|
| 2026-06-23 cyclic+iozone 对照 | 34.413713 | 34.261448 | 68.675161 | 34.426766 | 34.558952 | 68.985718 |
| cache retention + pruning | 37.410342 | 37.140767 | 74.551109 | 37.295328 | 36.504220 | 73.799548 |

产物:

- RV 日志: `docs/evidence/saved_logs/20260624-002017-rv-iozone-cache-retention-pruned/output_rv.txt`
- LA 日志: `docs/evidence/saved_logs/20260624-002649-la-iozone-cache-retention-pruned/output_la.txt`
- 评分报告: `docs/benchmarks/iozone/results/0624-0027-cache-retention-pruned.md`

完整性:

- RV 日志包含 2 个 iozone start/end 组，16 个 `iozone test complete.`
- LA 日志包含 2 个 iozone start/end 组，16 个 `iozone test complete.`

## 风险控制

这不是针对 iozone 名称的特判，而是把 close 后 clean page cache 的生命周期改得更接近 Linux。

不会改变这些语义:

- dirty 页在最后关闭时仍会写回。
- unlink/rename 覆盖目标时仍会走缓存失效路径。
- truncate/ftruncate 仍会清理超出新长度的缓存页。
- direct I/O 仍会在读前写回 dirty range，在写后失效 clean range。
- `drop_caches` 和卸载设备仍会主动丢缓存。

注意:

- 曾尝试用 `some_testcases` 跑 LTP 文件语义小集合，但 `some_testcases` 会触发 `init.sh` 只跑 LTP 的联动；第一轮格式也不正确，因此没有把它作为有效回归证据。最终 `src/init.sh` 已恢复为 `some_testcases=""`，没有保留临时配置。
