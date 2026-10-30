# cyclictest / iozone 修复与复测报告

生成时间: 2026-06-23 21:46 CST
最近更新: 2026-06-23 23:26 CST

## 最新结论

- 2026-06-23 22:26 的 RV `iozone-only` 复测出现 musl 低分，但不是稳定代码回归。
- 2026-06-23 23:26 使用同一份代码重新跑 RV `cyclictest+iozone`，iozone 已恢复到高分区间：glibc 34.413713，musl 34.261448。
- 本次没有修改内核或文件系统代码。原因是同一代码在对照条件下恢复，贸然修改文件系统/调度热路径反而更容易引入真实 bug。
- `src/init.sh` 中为了短测临时改过的测试开关已经恢复，最终没有留下本轮 iozone 复核的开关改动。

## 本轮范围

- 按要求，本轮定位 `cyclictest` / `iozone`，没有重新跑 LTP。
- `cyclictest+iozone` 短测阶段临时只打开 `cyclictest=1`、`iozone=1`，其它测试组为 0；日志里会出现被跳过测试组的空 start/end 标记。
- 后续为了复核 iozone 低分，又分别保存了 RV 全量中止日志、RV `iozone-only` 低分日志和 RV `cyclictest+iozone` 对照日志。
- 前一轮已经进入 LTP 的 RV 日志已中止并保存到 `docs/evidence/saved_logs/20260623-212727-aborted-ltp-before-cyclic-iozone-only/output_rv.txt`。

## 修复点

- LoongArch musl 的 `sched_get*` / `sched_set*` 补丁改为由 `loongarch_musl_sched_patches=1` 单独控制，用于修复 `cyclictest` 初始化实时调度策略时的 musl libc ENOSYS stub 问题。
- `compat_binary_patches=0` 保持关闭；这轮没有开启 LTP 相关兼容补丁。
- `cyclictest` 和 `iozone` 放在 LTP 前执行；本轮同时将 `ltp=0`，所以没有实际执行 LTP 用例。

## 运行命令

```bash
docker run --rm --privileged --platform linux/amd64 -w /work/scripts -v "$PWD":/work docker.io/zhouzhouyi/os-contest:20260510 bash -lc './run_rv.sh'
docker run --rm --privileged --platform linux/amd64 -w /work/scripts -v "$PWD":/work docker.io/zhouzhouyi/os-contest:20260510 bash -lc './run_la.sh'
```

## 日志与评分产物

- RV 当前日志: `docs/output_rv.txt`，内容为最近一次 RV `cyclictest+iozone` 复核。
- LA 短测日志以保存副本 `docs/evidence/saved_logs/20260623-214507-la-cyclic-iozone-only/output_la.txt` 为准；当前 `docs/output_la.txt` 已恢复到 LTP 场景。
- RV 保存副本: `docs/evidence/saved_logs/20260623-213713-rv-cyclic-iozone-only/output_rv.txt`
- LA 保存副本: `docs/evidence/saved_logs/20260623-214507-la-cyclic-iozone-only/output_la.txt`
- RV 全量中止副本: `docs/evidence/saved_logs/20260623-221946-aborted-full-rv-before-iozone-only/output_rv.txt`
- RV iozone-only 低分副本: `docs/evidence/saved_logs/20260623-222607-rv-iozone-only-regression-check/output_rv.txt`
- RV cyclictest+iozone 复核副本: `docs/evidence/saved_logs/20260623-232632-rv-cyclic-iozone-recheck/output_rv.txt`
- iozone 评分报告: `docs/benchmarks/iozone/results/0623-2145-cyclic-iozone-only.md`
- cyclictest 评分报告: `docs/benchmarks/cyclictest/results/0623-2146-cyclic-iozone-only.md`

## 完整性检查

| 架构 | libc | cyclictest | iozone | LTP |
|---|---|---:|---:|---:|
| RV | musl | 4/4 success | 8/8 complete | 0 cases |
| RV | glibc | 4/4 success | 8/8 complete | 0 cases |
| LA | musl | 4/4 success | 8/8 complete | 0 cases |
| LA | glibc | 4/4 success | 8/8 complete | 0 cases |

没有发现卡死项，也没有需要强制跳过的项。`iozone pwritev/preadv` 会打印 `Selected test not available on the version.`，但该段随后正常输出结果并以 `iozone test complete.` 结束。

## 分数汇总

| 架构 | libc | cyclictest 总分 | iozone 总分 |
|---|---|---:|---:|
| RV | musl | 4.000000 | 34.516144 |
| RV | glibc | 4.587540 | 32.940585 |
| LA | musl | 4.431794 | 34.558952 |
| LA | glibc | 4.795381 | 34.426766 |

## RV iozone 低分复核

| 日志 | glibc | musl | 合计 |
|---|---:|---:|---:|
| `20260623-213713-rv-cyclic-iozone-only` | 32.940585 | 34.516144 | 67.456729 |
| `20260623-222607-rv-iozone-only-regression-check` | 33.533636 | 30.088932 | 63.622568 |
| `20260623-232632-rv-cyclic-iozone-recheck` | 34.413713 | 34.261448 | 68.675161 |

低分日志主要掉在 musl 的 `random-read`、`read-backwards` 等多进程子项。评分脚本取 `Max throughput per process`，因此当 4 个进程吞吐较均匀时，分数可能低于一个进程吞吐很高、其它进程接近 0 的日志。复核日志已经回到后一种高分形态，说明这次低分更像运行形态波动，不适合作为代码修复依据。

和截图里的异常现象对比：

- `cyclictest` 的 LA musl 不再是 0，已经解析到 4/4 项并得到 4.431794。
- `iozone` 四个组合都在 32.94 到 34.56 之间；RV 两列相比截图中 26/27 分的状态明显恢复。

## 时间

- RV 测试阶段: 2026-06-23 13:30:54 到 13:37:02 UTC，约 6 分 08 秒。
- LA 测试阶段: 2026-06-23 13:39:15 到 13:44:50 UTC，约 5 分 35 秒。
- 两个架构的测试阶段合计约 11 分 43 秒。
- 编译日志显示 RV 构建 1 分 22 秒、LA 构建 1 分 19 秒；加上构建后可从日志确认的总耗时约 14 分 24 秒，未计入 Docker 启停的极小开销。
