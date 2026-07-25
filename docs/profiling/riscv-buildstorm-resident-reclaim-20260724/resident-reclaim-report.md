# RISC-V BuildStorm 驻留页回收完整检测报告

## 结论

本轮使用重新解压的官方 RISC-V 根文件系统，从 cagent 开始完整跑完
tg-xtask 预构建和正式 BuildStorm，并覆盖整个流程采集 CPU、off-CPU、
文件系统、页缓存、`mprotect` 以及 AddrSpace 写锁持有者数据。结果状态为
`complete`，没有 panic、死锁或采样丢失。

本轮可以确认三件事：

1. 按驻留 PTE 回收、整地址空间异步清理和多回收 worker，已经纠正了原先
   “按 1 MiB 虚拟地址反复切批”的明显退化。回收线程占非 idle CPU 的比例
   从约 30.8% 降到约 13.3%，AddrSpace 读写锁等待下降约 57%～58%。
2. `exit`、`exec` 和 `clone` 的累计持锁 task-time 明显下降。相对上一轮
   1 MiB 分批回收，预构建阶段分别下降 57.4%、64.0% 和 29.5%；正式阶段
   分别下降 64.1%、64.7% 和 40.0%。
3. 整体性能只恢复到历史最快 Moss 结果附近，还不能宣称已经获得稳定的
   全局提速。本轮正式编译为 582.76 秒，比错误的 1 MiB 分批回收快 1.32%，
   但比此前 per-VMA mprotect 一轮的 567.41 秒慢 2.71%。

最后一项比较还有一个重要限制：本轮是加入 RISC-V SBI RFENCE 远程 TLB
shootdown 后第一次完整测量；更早结果只有本地 TLB flush，SMP 语义并不完整。
因此 567.41 秒可以作为性能参照，但不能作为与本轮完全等价的正确性基线。

## 测试对象与构建修正

本次内核来自分支 `smp-scheduler-optimization`，以提交 `7ad5cc6b` 为工作树
基点，并包含当前尚未提交的 AddrSpace、缺页、回收、TLB 和探针修改。

正式测试前，profile 内核构建发现
`vendor/page_table_multiarch/src/bits64.rs` 中把 `Option<usize>` 直接用于
十六进制格式化。测试前已先用 `?` 取出实际 span，再据此判断页大小，并同步
更新 vendor checksum。随后 `make PROFILE=y kernel-rv` 完整通过。

根文件系统从
`/home/wuxun/Downloads/sdcard-rv-pub.img.gz` 重新解压到仓库中的
`sdcard-rv.img`。QEMU 使用 `-snapshot`，所以这次编译没有写回基础镜像。

## 测试和采样口径

| 项目 | 设置 |
|---|---|
| 架构 | RISC-V 64 |
| vCPU / 内存 | 8 / 8 GiB |
| QEMU | 宿主机直接运行，不使用 Docker |
| 根文件系统 | 官方全新解压镜像，`-snapshot` |
| 流程 | cagent → tg-xtask 预构建 → 正式 BuildStorm |
| CPU | 每 CPU 10 Hz |
| 通用 off-CPU | 1/8 抽样，累计时间乘 8 |
| ext4 | 1/16 抽样，累计时间乘 16 |
| page cache | 1/32 抽样，累计时间乘 32 |
| mprotect | 1/32 抽样，累计时间乘 32 |
| 普通 AddrSpace 持有者 | 1/16 抽样，累计时间乘 16 |
| exit / exec clear | 不抽样，每个完整清理区间精确记录 |
| clone | 父地址空间与子地址空间分别记录 |
| 丢样 | CPU、wait、pending、CPU try-lock 均为 0 |

“乘 8/16/32”只用于把抽中的区间还原成估算总 task-time；单个区间的最大值
仍然是实际测到的时间，不乘采样率。不同任务和不同地址空间可以同时执行，
因此 task-time 不能直接相加后当作墙钟时间。

完整 QEMU 命令、起止时间和宿主 CPU 记录见 [run.json](./run.json)，机器可读
统计见 [summary.json](./summary.json)。

## 正确性与完整性

- cagent 10/10 通过。
- tg-xtask 预构建完成，Cargo 自报 `22m 03s`。
- 正式 release 构建完成，Cargo 自报 `9m 24s`。
- `BUILDSTORM_COMPILE mode=multi ok=true`。
- 正式产物大小为 1,681,000 字节。
- profile 数据在 guest 停止采样后一次完整导出。
- 共生成 112 张 SVG 图。

日志仍会打印 `BUILDSTORM_MINIBUILD fail`，但随后的完整预构建和正式
BuildStorm 都成功。它是独立的预检失败，不能当作本轮正式编译失败；同时它
仍是需要单独修复的测试脚本问题。

## 完整耗时

| 阶段 | 本轮时间 |
|---|---:|
| setup | 4.72 s |
| tg-xtask 预构建 | 1,324.71 s |
| 正式 BuildStorm 阶段 | 659.52 s |
| 正式 cargo/axbuild 自报 | 582.76 s |
| 宿主完整墙钟 | 1,996.18 s |

### 与历史完整结果对比

| 系统/实现 | 预构建 | 正式阶段 | 正式编译自报 | 宿主完整墙钟 |
|---|---:|---:|---:|---:|
| Linux 同口径 | 919.05 s | 454.22 s | — | — |
| Moss per-VMA mprotect | 1,318.94 s | **637.58 s** | **567.41 s** | 1,981.23 s |
| Moss 1 MiB 分批回收 | 1,341.51 s | 665.51 s | 590.53 s | 2,026.23 s |
| **Moss 本轮驻留页回收** | **1,324.71 s** | **659.52 s** | **582.76 s** | **1,996.18 s** |

本轮相对 1 MiB 分批回收：

- 预构建快 1.25%；
- 正式阶段快 0.90%；
- 正式编译自报快 1.32%；
- 宿主完整墙钟快 1.48%。

本轮相对历史 per-VMA mprotect 一轮：

- 预构建慢 0.44%；
- 正式阶段慢 3.44%；
- 正式编译自报慢 2.71%；
- 宿主完整墙钟慢 0.75%。

本轮 Moss 的预构建时间是 Linux 的 1.441 倍，正式阶段是 Linux 的
1.452 倍。Linux 仍然有约 31% 的阶段墙钟优势。

## CPU 火焰图

核心图：

- [预构建 CPU 火焰图](./guest-cpu-prebuild.svg)
- [正式阶段 CPU 火焰图](./guest-cpu-timed.svg)
- [完整 CPU 火焰图](./guest-cpu-all.svg)

| 阶段 | CPU 样本 | idle | 非 idle | 回收 worker | 回收占非 idle | 缺页路径占非 idle |
|---|---:|---:|---:|---:|---:|---:|
| 预构建 | 41,107 | 27,184 | 13,923 | 1,880 | **13.50%** | **31.86%** |
| 正式阶段 | 25,499 | 19,467 | 6,032 | 795 | **13.18%** | **32.10%** |

这里的 idle 比例高并不表示整个编译始终只有一核。宿主记录显示依赖图中可运行
crate 足够多时，QEMU 长时间达到约 770%～796% CPU；进入依赖图尾部后，只剩
单个 `rustc`、`rustls`、`axbuild` 或 `tg-xtask` 关键路径时，CPU 会自然降到
约一到数核。CPU 火焰图把两种时期合并在同一阶段内。

### 回收路径

上一轮 1 MiB 分批实现中，回收线程分别占两个阶段非 idle 样本的 30.77% 和
30.74%。本轮降到 13.50% 和 13.18%，说明去掉反复切 VMA、反复入队和按稀疏
虚拟范围扫描是有效的。

本轮回收 worker 中：

| 阶段 | 回收样本 | SBI RFENCE | RFENCE 占回收样本 |
|---|---:|---:|---:|
| 预构建 | 1,880 | 373 | 19.84% |
| 正式阶段 | 795 | 137 | 17.23% |

当前回收路径里最明确的新成本，是每页或小范围 unmap 触发的
`flush_tlb -> sbi_call_4`。它保证 RISC-V 多核 TLB 一致性，不能重新退回只刷
本核；后续应做的是在回收批次末尾合并 shootdown，而不是省略远程 flush。

### 缺页路径

`handle_page_fault_concurrent` 仍是最大的可识别内核 CPU 路径：

| 阶段 | 缺页路径样本 | Cow execute | commit | 分配页框 |
|---|---:|---:|---:|---:|
| 预构建 | 4,436 | 1,628 | 1,369 | 311 |
| 正式阶段 | 1,936 | 727 | 623 | 105 |

这说明 AddrSpace 写锁已经不再是唯一问题。缺页的锁外 execute、COW 元数据、
页框分配和锁内 commit 本身仍消耗大量 CPU。下一轮应继续细分：

- COW execute 中的复制、引用计数和 per-CPU 页分配；
- commit 中页缓存查找、PTE 安装和 stale retry；
- 文件缺页的缓存命中、folio 等待和真实块 I/O；
- 单页 TLB flush 与可合并的批量 flush。

## AddrSpace 读写锁等待

从 workload off-CPU 图按最终阻塞点聚合：

| 阶段 | 实现 | 读锁等待 | 写锁等待 | 合计 |
|---|---|---:|---:|---:|
| 预构建 | 1 MiB 分批回收 | 176.84 s | 14.20 s | 191.05 s |
| 预构建 | **本轮** | **77.02 s** | **5.62 s** | **82.64 s** |
| 正式 | 1 MiB 分批回收 | 87.96 s | 8.67 s | 96.63 s |
| 正式 | **本轮** | **38.14 s** | **2.13 s** | **40.27 s** |

本轮相对分批回收，预构建合计下降 56.74%，正式阶段下降 58.33%。相对分批
回收之前的 98.06 秒和 64.45 秒，也分别下降约 15.73% 和 37.52%。

这项结果比总墙钟更直接地验证了当前改动的机制：退出和 exec 的死亡地址空间
不再由一个高频 writer 反复进入同一把写锁，新 reader 被 writer-preference
锁挡住的时间明显减少。

图表：

- [预构建 workload off-CPU](./guest-offcpu-workload-prebuild.svg)
- [正式 workload off-CPU](./guest-offcpu-workload-timed.svg)
- [预构建锁等待](./guest-offcpu-lock-prebuild.svg)
- [正式锁等待](./guest-offcpu-lock-timed.svg)

## AddrSpace 写锁持有者

### 预构建

普通持有者的“估算区间”是抽中次数乘 16。`exit` 和 `exec` 是完整地址空间
清理的精确次数，不再是旧实现中数十万次 1 MiB 清理批次。

| 持有者 | 累计 task-time | 区间数 | 平均区间 | 最大区间 |
|---|---:|---:|---:|---:|
| `exit clear` | **130.72 s** | 1,587 精确 | 82.37 ms | **2,252.25 ms** |
| `clone child` | **114.39 s** | 1,040 估算 | 109.99 ms | 233.79 ms |
| `exec clear` | **90.96 s** | 522 精确 | 174.25 ms | 425.18 ms |
| `clone parent` | **59.95 s** | 544 估算 | 110.21 ms | 196.05 ms |
| `mprotect` | 58.35 s | 1,469,216 估算 | **0.0397 ms** | 2.68 ms |
| `munmap` | 43.14 s | 50,224 估算 | 0.859 ms | 104.04 ms |
| `madvise` | 12.07 s | 4,384 估算 | 2.753 ms | 71.49 ms |
| `mmap` | 4.86 s | 104,864 估算 | 0.0464 ms | 2.11 ms |
| 其他 | 1.48 s | 32,720 估算 | 0.0451 ms | 4.75 ms |
| `mremap` | 0.06 s | 1,216 估算 | 0.0512 ms | 0.21 ms |

### 正式阶段

| 持有者 | 累计 task-time | 区间数 | 平均区间 | 最大区间 |
|---|---:|---:|---:|---:|
| `clone child` | **50.59 s** | 688 估算 | 73.53 ms | 132.79 ms |
| `exit clear` | **48.99 s** | 481 精确 | 101.85 ms | **1,090.81 ms** |
| `exec clear` | **40.71 s** | 341 精确 | 119.39 ms | 309.41 ms |
| `mprotect` | 25.27 s | 661,488 估算 | **0.0382 ms** | 27.04 ms |
| `clone parent` | 24.63 s | 320 估算 | 76.98 ms | 137.47 ms |
| `munmap` | 14.84 s | 21,696 估算 | 0.684 ms | 28.01 ms |
| `madvise` | 4.42 s | 2,320 估算 | 1.905 ms | 21.87 ms |
| `mmap` | 1.44 s | 32,240 估算 | 0.0447 ms | 0.57 ms |
| 其他 | 0.83 s | 13,152 估算 | 0.0631 ms | 8.62 ms |
| `mremap` | 0.02 s | 480 估算 | 0.0485 ms | 0.12 ms |

`clone child` 每次 fork 通常会出现两个区间：`try_clone()` 内安装复制映射，
以及返回后执行 `copy_from_kernel` / `wipeonfork` 的子地址空间更新。因此它的
区间数不是进程数，父、子两行的累计时间也属于可重叠 task-time。

相对上一轮未区分父子、且采用 1 MiB 分批清理的结果：

| 操作 | 预构建变化 | 正式阶段变化 |
|---|---:|---:|
| exit clear | **-57.36%** | **-64.05%** |
| exec clear | **-64.03%** | **-64.68%** |
| clone parent + child | **-29.49%** | **-40.02%** |

`exit clear` 最大值仍达到 2.25 秒，但它发生在已经死亡、由后台 worker 独占的
地址空间上，不会像同步退出那样直接阻塞原进程。它仍会消耗 CPU、页表锁和
RFENCE，所以是吞吐问题，不再是活地址空间长时间被同一把锁阻塞的问题。

核心图：

- [exit clear 预构建](./guest-aspace-exit-clear-write-hold-prebuild.svg)
- [exec clear 预构建](./guest-aspace-exec-clear-write-hold-prebuild.svg)
- [clone parent 预构建](./guest-aspace-clone-parent-write-hold-prebuild.svg)
- [clone child 预构建](./guest-aspace-clone-child-write-hold-prebuild.svg)
- [munmap 预构建](./guest-aspace-munmap-write-hold-prebuild.svg)
- [mprotect 预构建](./guest-aspace-mprotect-write-hold-prebuild.svg)

## mprotect

| 指标 | 预构建 | 正式阶段 |
|---|---:|---:|
| 精确调用数 | 1,469,205 | 661,489 |
| 总 task-time | 78.58 s | 33.99 s |
| 平均总耗时 | 53.48 µs | 51.39 µs |
| AddrSpace 锁等待 | 0.77 s | 1.16 s |
| 本地更新 | 41.79 s | 17.87 s |
| 平均本地更新 | 28.44 µs | 27.02 µs |
| 写锁平均持有 | 39.72 µs | 38.21 µs |
| record/seal 全局锁 | 0 | 0 |

每次调用仍然只验证、访问一个 VMA；旧的全局 `MMAP_RECORDS` 和 seal 扫描
没有重新出现。与 per-VMA mprotect 一轮相比，本轮 AddrSpace 锁等待预构建
下降 59.0%，正式阶段下降 24.0%，而本地更新只增加约 2%～3%。

`mprotect-total` 增加约 19%～21%，但这两轮并非完全等价：本轮新增正确的
RISC-V 多核远程 TLB shootdown。当前证据不足以把全部增量都归因于 RFENCE，
不过不能为了回到旧数字而恢复只刷新本核的错误实现。下一步应单独增加
TLB shootdown 请求数、覆盖页数、合并数和 SBI 耗时探针。

图表：

- [mprotect 总耗时预构建](./guest-mprotect-total-prebuild.svg)
- [mprotect 本地更新预构建](./guest-mprotect-local-update-prebuild.svg)
- [mprotect AddrSpace 等待预构建](./guest-mprotect-aspace-lock-wait-prebuild.svg)

## 文件系统、页缓存和同步 I/O

以下都是累计 task-time，不是阶段墙钟：

| 事件 | 预构建 | 正式阶段 | 最大实测区间（预构建 / 正式） |
|---|---:|---:|---:|
| page-cache inclusive | 161.66 s | 45.94 s | 91.31 / 17.45 ms |
| ext4 inclusive | 31.96 s | 49.42 s | 49.97 / 57.05 ms |
| ext4 全局锁等待 | 7.50 s | 1.40 s | 229.11 / 86.86 ms |
| folio load 等待 | 4.02 s | 0.59 s | 97.03 / 94.58 ms |
| 同步块读 | 7.01 s | 2.26 s | 99.84 / 51.52 ms |
| 同步块写 | 0.71 s | 0.35 s | 49.89 / 9.36 ms |
| 通用 mutex 等待 | 8.35 s | 1.54 s | 152.56 / 86.84 ms |

与错误的 1 MiB 分批回收相比，本轮预构建 page-cache inclusive 从
213.80 秒降到 161.66 秒，ext4 全局锁等待从 14.36 秒降到 7.50 秒。这与
回收 worker CPU 和 AddrSpace 等待同时下降，说明旧分批方案确实在制造额外
页缓存、文件引用释放和锁竞争，而不是单纯受到 crate 顺序随机波动影响。

ext4 全局锁仍存在，但其累计等待量已经显著小于缺页、回收和地址空间复制
task-time。现阶段先继续拆 ext4 大锁，不一定比批量 TLB flush、COW commit
以及 clone 元数据复制更有收益。

相关图：

- [ext4 锁等待预构建](./guest-ext4-lock-wait-prebuild.svg)
- [folio load 等待预构建](./guest-folio-load-wait-prebuild.svg)
- [文件系统 I/O 延迟预构建](./guest-fs-io-latency-prebuild.svg)

## 下一步优化顺序

### 1. 合并死亡地址空间回收的 TLB shootdown

这是当前证据最直接的下一项。回收 worker 中约 17%～20% CPU 样本落在
SBI RFENCE。应让 unmap 先收集实际失效的地址区间，在释放页面引用之前或批次
结束时一次提交合并后的 shootdown，并在确认所有 hart 完成后再复用物理页。
必须保留远程一致性，不能只做本地 `sfence.vma`。

验收指标：

- 每个 exit/exec 的 SBI 调用次数；
- 每次 shootdown 覆盖页数；
- 合并前后回收 worker 中 SBI 样本；
- 回收任务 CPU 比例是否从约 13% 继续下降；
- 不出现旧 PTE、COW 或物理页提前复用错误。

### 2. 细分并优化 COW 缺页 execute/commit

缺页路径占非 idle CPU 约 32%，已经大于回收线程。应为 prepare、锁外
execute、commit、stale retry、页分配和引用表操作分别增加 CPU/时间计数，
而不是再用一个总缺页事件猜测。

优先检查：

- `CowFaultPlan::execute` 的复制和页分配；
- `CowBackend::commit_fault` 的页缓存/BTree 查找；
- 相同页并发缺页造成的重复 execute；
- stale 后已完成工作能否安全复用；
- 4 KiB fault 是否可以从已加载 folio 直接安装引用。

### 3. 继续拆 clone，而不是重新拆 mprotect

clone 父、子地址空间单个区间仍约 74～110 ms。当前已经证明 mprotect 写锁
平均只有约 38～40 µs，继续优化 mprotect 不会消除这种百毫秒级长区间。

clone 的下一步应把：

- 子页表分配；
- VMA/backend 元数据复制；
- 文件页和 COW 引用准备；

移到父地址空间锁外。父 PTE 从 writable 改成 COW 仍需要受控 commit 和批量
TLB shootdown，不能简单并发遍历并原地修改父页表。

### 4. 最后再评估 ext4 全局锁拆分

ext4 锁等待仍有长尾，但累计值已下降。只有在完成 TLB/COW/clone 的细分后，
若 workload off-CPU 重新证明 ext4 是主要可运行任务的阻塞点，才继续按
inode、block cache 和目录操作拆锁。否则容易再次出现“局部结构更复杂，
完整墙钟反而变慢”。

## 最终判断

本轮修改没有再造成上一次 1 MiB 分批回收的性能灾难，也没有出现卡死。
机制层面已经有明确、可重复解释的改善：

- 回收 CPU 占比约减半；
- AddrSpace 锁等待约减半；
- exit/exec/clone 持锁 task-time 显著下降；
- 完整编译相对错误分批版本恢复约 1%～1.5%。

但整体耗时尚未稳定快于历史最快 Moss 一轮，更没有追平 Linux。当前最大的
已识别内核 CPU 路径已转移到 COW 缺页 execute/commit，其次是带远程 RFENCE
的异步回收；这两项应成为下一轮优化和探针设计的中心。
