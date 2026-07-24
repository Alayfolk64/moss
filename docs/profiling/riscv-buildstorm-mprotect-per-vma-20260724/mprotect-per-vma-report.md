# RISC-V 干净镜像 `mprotect` per-VMA 优化验收报告

## 结论

这次结果不是上一次失败测试留下的 `target` 缓存造成的。

测试前重新从
`/home/wuxun/Downloads/sdcard-rv-pub.img.gz` 解压生成了仓库根目录的
`sdcard-rv.img`，并在启动 QEMU 前确认：

- `/work/tgoskits/target` 不存在；
- `/work/tgoskits/tg-xtask/target` 不存在；
- 镜像 SHA-256 为
  `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0`；
- QEMU 使用 `-snapshot`，测试结束后再次检查，上述两个目录仍然不存在。

镜像内存在官方预置的 Cargo registry/git **源码缓存**，但不存在本次工程的
编译产物缓存。这与评测镜像的正常起点一致。

在这个干净起点上，完整 RISC-V 流程结果为：

- cagent：10/10 通过；
- tg-xtask 完整预构建：成功，Cargo 自报 21 分 57 秒；
- 正式 BuildStorm：成功，567.41 秒，即 9 分 27.41 秒；
- profiling 全流程宿主墙钟：1981.23 秒，即 33 分 01.23 秒；
- panic：无；
- profile 丢样：全部为 0。

与 2026-07-24 修改前的同口径完整 profiling 基线相比：

| 指标 | 修改前 | 干净镜像修改后 | 减少 |
|---|---:|---:|---:|
| tg-xtask prebuild phase | 39 分 23.83 秒 | 21 分 58.94 秒 | **44.20%** |
| 正式 BuildStorm | 20 分 43.89 秒 | 9 分 27.41 秒 | **54.38%** |
| timed phase（含收尾） | 22 分 01.04 秒 | 10 分 37.58 秒 | **51.74%** |
| 宿主全流程墙钟 | 61 分 46.29 秒 | 33 分 01.23 秒 | **46.54%** |

调用规模并没有因为缓存而消失：预构建仍执行了 1,473,748 次
`mprotect`，比旧基线的 1,421,438 次还多 3.68%；正式阶段执行 658,183
次，比旧基线的 650,266 次多 1.22%。约 74% 的调用仍然只处理一个 4 KiB
页。调用量与工作形态都说明这是一轮完整冷 `target` 构建。

最关键的结果是，修改前两个全局 `MMAP_RECORDS` 锁等待、两次全表扫描和
全表重建已经从热点中消失。`mprotect` 估算累计任务时间下降 99.27%～
99.34%，而正式 BuildStorm 墙钟下降 54.38%。这说明原报告定位的全局 VMA
记录锁确实是造成并行编译串行化的主因，而不只是火焰图上的相关现象。

## 测试对象

### 干净镜像

| 项目 | 值 |
|---|---|
| 压缩镜像 | `/home/wuxun/Downloads/sdcard-rv-pub.img.gz` |
| 压缩镜像 SHA-256 | `33941fbfd53e11a1c9e9f72dfafcc27641c9db445c9b48020a9e56e1539d4a8f` |
| 解压镜像 | `/home/wuxun/Projects/moss/sdcard-rv.img` |
| 解压镜像大小 | 15,032,385,536 字节 |
| 解压镜像 SHA-256 | `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0` |
| 初始 tg-xtask target | 不存在 |
| 初始 tg-xtask/target | 不存在 |
| 测试后两个 target | 仍不存在，证明 `-snapshot` 未写回基础镜像 |

### QEMU 和内核

- 架构：RISC-V 64；
- QEMU machine：`virt`；
- vCPU：8；
- 内存：8 GiB；
- 块设备、网络、BIOS：与 `run_rv.sh` 相同；
- 磁盘模式：`-snapshot`；
- profiling kernel SHA-256：
  `d5ce5229a4c30b4ea6376df49f5355ec6f01673742ce4a3ac81f11b71c9d312c`；
- profiling ELF SHA-256：
  `9ba4d3341975e20f44a66b38da1e13293fff2a425936f023a6d593387b78b954`；
- 起止时间：2026-07-24 14:47:16～15:20:17；
- runner 状态：`complete`。

实际命令、每 15 秒 QEMU CPU 利用率和导出地址见
[run.json](./run.json)。主要并行编译区间的宿主 QEMU CPU 大多处于
760%～796%，说明 8 个 vCPU 确实同时得到宿主计算时间。依赖图解析、串行
链接和同步阶段降到约 100%～600% 属于任务阶段差异，不能仅凭这些区间认定
调度器退回单核。

## 本轮实现

### 1. VMA 元数据归属每个 `MemoryArea`

`MemoryArea` 现在直接携带调用方定义的元数据：

```rust
pub struct MemoryArea<B: MappingBackend, M = ()> {
    va_range: AddrRange<B::Addr>,
    flags: B::Flags,
    backend: B,
    metadata: M,
}
```

Moss 的 `AddrSpace` 使用 `MemorySet<Backend, VmaMetadata>`。每个 VMA
保存：

- 用户实际请求的 `user_flags`；
- Linux `VM_MAY*` 语义对应的 `may_flags`；
- shared/file-backed/anonymous/locked 属性。

因此 `mprotect` 不再访问全系统共享的 `MMAP_RECORDS` 来询问“这段映射原来
是什么、能不能加写权限”。映射属性和生命周期回到所属地址空间。

### 2. `may_flags` 在建图时决定权限上限

共享只读文件和 write-sealed 文件在 `mmap` 时就从 `may_flags` 移除
`WRITE`。`mprotect(PROT_WRITE)` 只检查命中的 VMA 本地元数据，不再扫描
所有进程的映射记录做一次昂贵的否定查询。

这对应 Linux 将“当前权限”和“以后最多能提升到什么权限”分开保存的
`VM_READ/WRITE/EXEC` 与 `VM_MAYREAD/MAYWRITE/MAYEXEC` 思路。

### 3. 从目标地址开始范围查询

`MemorySet::protect` 先用 BTreeMap 的 predecessor lookup 找到包含
`start` 的 VMA，然后沿目标范围前进。它不再从本进程第一个 VMA开始，更
不会从全系统第一个映射记录开始。

验证阶段先确认整个范围无空洞且权限允许；验证完成后，更新阶段只重访已经
证明覆盖目标区间的 VMA。页表修改仍然可能失败，因此代码在原 VMA 还留在
树中时先执行唯一可能失败的操作，成功后才做不会失败的拆分和元数据更新。
这样错误返回不会把 VMA 从树里丢掉。

### 4. 权限未变化快速返回

如果范围内所有 VMA 的页表权限和 `user_flags` 都已经等于请求值，
`protect` 在验证后直接返回，不拆 VMA、不写 PTE、不触发 TLB 刷新。

本轮 BuildStorm 中 `vmas_unchanged=0`，说明这个特定负载没有命中完整
no-op 快路径。它已经实现并经过静态检查，但不能把本轮提速归功于这一项。

### 5. 相邻 VMA 合并

权限更新造成边界拆分后，如果相邻 VMA 的权限、元数据和 backend 连续性都
兼容，就立即合并。预构建阶段 1,473,711 次拆分后完成 1,449,914 次合并，
抵消率 98.39%；正式阶段为 98.52%。

这阻止了百万次小范围 `mprotect` 把单个地址空间永久碎片化成数万条记录。

## 静态审查

按要求，编译和运行前完成了三轮独立侧重点的静态审查：

1. 数据结构和调用方审查：覆盖 `mmap`、`mprotect`、`mremap`、
   `madvise`、`munmap`、`fork/clone`、`exec`、退出和 `/proc/maps`；
2. 并发与语义审查：检查 write seal、共享地址空间、`vfork/CLONE_VM`、
   `mlock` 计数及 fork 不继承 locked 状态；
3. 错误与边界审查：检查空洞、溢出、零长度、页表更新失败、VMA 拆分/合并、
   profiler ABI 和 vendor 校验和。

审查中实际修复了三个不能靠性能测试掩盖的问题：

- 页表更新失败时原 VMA 可能已经从树中移除；
- no-op 在第二遍可能仍制造无意义的边界拆分；
- 共享地址空间最后一个 owner 和普通 fork 的 locked accounting 不同。

此外，COW backend 的映射页计数改为共享原子计数，保证 VMA 拆分后的 backend
克隆不会各自维护一份失真的计数。

## 测量口径

### 精确计数

以下数据逐调用计数，不抽样：

- 调用、成功、失败；
- 请求页数和大小直方图；
- 权限组合；
- 验证、访问、修改、未修改 VMA 数；
- 拆分、合并和操作结束时 VMA 数；
- 单次最大验证/访问 VMA 数。

精确统计存储在独立的 960 字节 `mprotect-stats.bin` 中。

### 抽样耗时

| 数据 | 口径 |
|---|---:|
| guest CPU | 10 Hz |
| off-CPU | 1/8 抽样，累计耗时乘 8 |
| ext4 | 1/16 抽样，累计耗时乘 16 |
| page cache | 1/32 抽样，累计耗时乘 32 |
| mprotect 子事件 | 每 CPU、每事件 1/32 抽样，累计耗时乘 32 |

乘数用于把抽中的样本外推为累计任务时间。它不是“把程序执行时间乘长”，也
不是精确事件数。`max_sample` 只代表被抽中的最大样本。精确调用/VMA 计数
不乘任何倍数。

`task-s` 是所有任务耗时之和，不是墙钟时间。多个任务同时等待时，累计
`task-s` 可以超过阶段墙钟。

### 新旧事件对应

修改前将 `mprotect` 拆为：

- AddrSpace 锁等待；
- seal 全局记录锁等待和扫描；
- 页表权限更新；
- record 全局记录锁等待和全表重建。

修改后保留端到端时间和 AddrSpace 锁等待，并将局部 VMA 验证、PTE 更新、
拆分和合并统称为 `mprotect-local-update`。四个旧的全局 record/seal 事件
仍保留事件编号用于对比，但本轮精确为 0。

## 精确 VMA 行为

| 指标 | 预构建 | 正式阶段 |
|---|---:|---:|
| `mprotect` 调用 | 1,473,748 | 658,183 |
| 成功 / 失败 | 1,473,748 / 0 | 658,183 / 0 |
| 请求页数 | 7,513,028 | 3,811,943 |
| 平均页数/调用 | 5.10 | 5.79 |
| 最大页数/调用 | 2873 | 3104 |
| 只请求 1 页 | 1,091,967（74.09%） | 485,681（73.79%） |
| `READ\|WRITE` | 1,455,120（98.74%） | 651,607（99.00%） |
| 验证 VMA | 1,473,748 | 658,183 |
| 单次最大验证 VMA | **1** | **1** |
| 实际访问 VMA | 1,473,748 | 658,183 |
| 单次最大访问 VMA | **1** | **1** |
| 修改 / 未修改 VMA | 1,473,748 / 0 | 658,183 / 0 |
| 拆分 | 1,473,711 | 658,155 |
| 合并 | 1,449,914 | 648,390 |
| 合并/拆分 | 98.39% | 98.52% |
| 每次操作结束时 VMA 数平均值 | 124.70 | 94.18 |

“单次最大验证 VMA=1”不是硬编码限制，而是本负载的每次 `mprotect` 都落在
同一条 VMA 内。范围查询支持跨多条连续 VMA；这里的统计证明它只处理实际
重叠区域。

旧实现的预构建每次 update 平均扫描 31,763 条全系统记录、单次最多扫描
83,638 条；正式阶段平均扫描 31,568 条、最多 71,108 条。新实现把这项工作
降为本轮每次 1 条 VMA。

## `mprotect` 耗时对比

### 预构建

| 指标 | 修改前 | 修改后 | 变化 |
|---|---:|---:|---:|
| 调用数（精确） | 1,421,438 | 1,473,748 | +3.68% |
| 端到端累计 task-s | 8931.02 | 65.13 | **-99.27%** |
| 端到端平均/调用 | 6.283 ms | 44.19 µs | **-99.30%** |
| AddrSpace 锁等待 task-s | 97.89 | 1.89 | **-98.07%** |
| seal 全局锁等待 | 3565.99 | 0 | 消除 |
| seal 全表扫描 | 301.89 | 0 | 消除 |
| record 全局锁等待 | 3025.80 | 0 | 消除 |
| record 全表重建 | 1365.60 | 0 | 消除 |
| 旧页表更新 / 新局部更新 | 418.04 | 40.57 | **-90.29%** |
| 更新平均/调用 | 294.10 µs | 27.53 µs | **-90.64%** |

### 正式阶段

| 指标 | 修改前 | 修改后 | 变化 |
|---|---:|---:|---:|
| 调用数（精确） | 650,266 | 658,183 | +1.22% |
| 端到端累计 task-s | 4312.28 | 28.55 | **-99.34%** |
| 端到端平均/调用 | 6.632 ms | 43.38 µs | **-99.35%** |
| AddrSpace 锁等待 task-s | 55.65 | 1.53 | **-97.25%** |
| seal 全局锁等待 | 1755.63 | 0 | 消除 |
| seal 全表扫描 | 131.07 | 0 | 消除 |
| record 全局锁等待 | 1409.25 | 0 | 消除 |
| record 全表重建 | 639.57 | 0 | 消除 |
| 旧页表更新 / 新局部更新 | 224.80 | 17.48 | **-92.22%** |
| 更新平均/调用 | 345.70 µs | 26.56 µs | **-92.32%** |

“旧页表更新”和“新局部更新”不是完全相同的计时边界：新事件还包含 VMA
验证、拆分和合并，因此它的范围更宽。即便如此，新值仍低约 90%～92%。

### 新实现内部

| 阶段 | total task-s | local update task-s | AddrSpace 等待 task-s | 抽中最大 total | 抽中最大 local update |
|---|---:|---:|---:|---:|---:|
| 预构建 | 65.13 | 40.57 | 1.89 | 14.436 ms | 0.707 ms |
| 正式 | 28.55 | 17.48 | 1.53 | 15.713 ms | 0.664 ms |

少量端到端长尾不在局部 VMA 更新里：它还可能包含调度、抢占和获取
AddrSpace 锁。局部更新被抽中的最大值低于 1 ms，说明旧的多毫秒全表扫描/
重建长尾已经消失。

## 火焰图怎么看

最直接的验收图是：

- [修改后预构建 mprotect 总耗时](./guest-mprotect-total-prebuild.svg)
- [修改后正式阶段 mprotect 总耗时](./guest-mprotect-total-timed.svg)
- [修改后预构建局部更新](./guest-mprotect-local-update-prebuild.svg)
- [修改后正式阶段局部更新](./guest-mprotect-local-update-timed.svg)
- [修改后预构建 AddrSpace 等待](./guest-mprotect-aspace-lock-wait-prebuild.svg)
- [修改后正式阶段 AddrSpace 等待](./guest-mprotect-aspace-lock-wait-timed.svg)

对照旧报告：

- [修改前预构建 seal 全局锁等待](../riscv-buildstorm-mprotect-20260724/guest-mprotect-seal-lock-wait-prebuild.svg)
- [修改前预构建 record 全局锁等待](../riscv-buildstorm-mprotect-20260724/guest-mprotect-record-lock-wait-prebuild.svg)
- [修改前预构建 record 全表更新](../riscv-buildstorm-mprotect-20260724/guest-mprotect-record-update-prebuild.svg)

修改后的四类旧事件图只有空图框，这是统计值为 0 的正常表示，不是导出
失败。新的主要栈集中在 `sys_mprotect -> AddrSpace::protect ->
MemorySet::protect`，宽度是数十秒累计任务时间；旧图中两把全局记录锁和
全表 drain/rebuild 是数千秒累计任务时间。

完整 CPU 图：

- [预构建 CPU](./guest-cpu-prebuild.svg)
- [正式阶段 CPU](./guest-cpu-timed.svg)

旧 CPU 图里 `sys_mprotect` 后面的 BTree/Vec 遍历是显著热点；新图的 CPU
热点已经转向 COW 缺页、进程地址空间回收、clone/unmap、ext4 CRC 和内存
清零。CPU 图中的 `task:idle` 宽度不应直接解释为宿主 CPU 空闲比例：它是
采样时 Moss 当前任务标签与栈归类，和 `run.json` 记录的 QEMU 宿主进程
CPU 百分比不是同一个指标。

## 剩余瓶颈

本轮 `mprotect` 已不再是主瓶颈，但不能据此说整个内存管理和文件系统都已
优化完成。

### 1. 构建阶段本身仍有串行区间

宿主采样显示大部分 crate 并行编译时 QEMU 接近 8 核满载，但依赖解析、
链接/LTO、Cargo 协调和末尾同步会自然下降。off-CPU workload 图的主要栈
已经是 rustc/tokio 的 futex、Cargo poll 和协调线程等待，而不是
`MMAP_RECORDS` mutex：

- [预构建 workload off-CPU](./guest-offcpu-workload-prebuild.svg)
- [正式阶段 workload off-CPU](./guest-offcpu-workload-timed.svg)

这些等待中既包含合理的线程池空闲，也包含可能继续优化的调度/唤醒成本，
不能把所有 futex 等待都当作内核缺陷。

### 2. ext4 全局锁仍存在，但量级远小于旧 mprotect 锁

修改后 ext4 全局锁等待估算为：

- 预构建 6.52 task-s；
- 正式阶段 1.23 task-s。

热点主要来自 readahead worker 的 extent 读取，以及 lookup/stat/read 路径：

- [预构建 ext4 锁等待](./guest-ext4-lock-wait-prebuild.svg)
- [正式阶段 ext4 锁等待](./guest-ext4-lock-wait-timed.svg)

这些值比修改前 `mprotect` 的 8931/4312 task-s 小三个数量级。因此下一步
可以继续拆 ext4 全局锁，但不能再把它当成本轮 20 分钟正式编译的主要解释。

### 3. 缺页和地址空间销毁进入 CPU 可见热点

新 CPU 图中可见 COW fault execute、`unmap_area`、`clone_map` 和内存清零。
这是消除 `mprotect` 巨大热点后暴露出来的下一层成本。后续优化应优先用
现有 CPU/off-CPU 图量化：

- COW fault 的页分配、复制和锁释放；
- 进程退出时逐 VMA/PTE 回收；
- fork/clone 的映射复制；
- 页面清零和 ext4 CRC。

不应在没有新增分段测量前继续改动这些路径，因为它们每一项在当前 CPU 图中
都远小于旧 `mprotect` 热点。

## 功能和构建结果

| 检查 | 结果 |
|---|---|
| 三轮静态审查 | 完成 |
| profiling RISC-V kernel build | 成功 |
| vendor checksum | 已更新并验证 |
| Python profiling scripts 语法 | 通过 |
| cagent | 10/10 通过 |
| tg-xtask 完整预构建 | 成功 |
| 正式 BuildStorm | 成功 |
| 正式产物 | 1,681,000 字节 |
| panic | 无 |
| profile dump | 第一次成功 |
| dropped/skipped profile | 全部为 0 |

`BUILDSTORM_MINIBUILD` 仍输出 `fail`，但随后完整 tg-xtask 预构建和正式
BuildStorm 均成功。这个既有的探针结果与本轮 `mprotect` 修改无关，不能
用来否定正式构建结果；也不能把它隐去，后续应单独修复 minibuild 探针或其
前置目录假设。

## 复现与原始数据

核心数据文件：

- [runner 元数据](./run.json)
- [解析后的完整统计](./summary.json)
- [串口日志](./console.log)
- [完整 profiling 解析日志](./profile.log)

修改前基线和测量解释：

- [修改前 mprotect 详细报告](../riscv-buildstorm-mprotect-20260724/mprotect-report.md)
- [修改前完整 summary](../riscv-buildstorm-mprotect-20260724/summary.json)

`summary.json` SHA-256：
`923b65a06bcf85cbbd8206a4b46805596149634370d1e5cbf46592bbf8550f8f`。

本报告只把有相同定义的指标做直接数值比较。新 `local-update` 比旧
`page-table` 覆盖范围更宽；CPU 火焰图受阶段长度变化影响，宽度应优先看
百分比和调用路径，不能直接拿样本绝对数判断回归。
