# RISC-V 新分配用户页 Zicboz 清零迭代报告

## 结论

本轮修改有效降低了新分配用户页的清零开销，而且修改范围严格限制在尚未发布的新 frame，没有重做此前会破坏文件系统的全局 `memset` 优化。

在相同官方镜像、8 GiB、8 vCPU、`-snapshot` 和完整 profiler 口径下：

| 指标 | 原子 usage 基线 | Zicboz 清零 | 变化 |
| --- | ---: | ---: | ---: |
| Cargo 预构建 | 18m56s | **18m37s** | -19s |
| prebuild phase | 1137.340 s | **1118.045 s** | -19.295 s（-1.70%） |
| Cargo release | 8m14s | **8m13s** | -1s |
| BuildStorm | 509.12 s | **507.87 s** | -1.25 s（-0.25%） |
| timed phase | 574.776 s | **572.289 s** | -2.487 s（-0.43%） |
| 宿主全流程 | 1726.348 s | **1711.345 s** | -15.003 s（-0.87%） |

端点时间只有一次完整运行，1.25 秒的 BuildStorm 变化不能单独当作稳定收益。目标 CPU 路径的变化更直接：

| CPU 计数 | 基线预构建 | Zicboz 预构建 | 变化 | 基线正式 | Zicboz 正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 旧 `memset` | 643 | 129 | -514 | 264 | 75 | -189 |
| 新 `zero_fresh_frame` | 0 | 70 | +70 | 0 | 29 | +29 |
| 保守合并清零宽度 | **643** | **199** | **-69.1%** | **264** | **104** | **-60.6%** |
| 非 idle 样本 | 8255 | 7713 | -6.57% | 3777 | 3607 | -4.50% |

这里把新版本残留的全部 `memset` 与 `zero_fresh_frame` 相加，是有意采用保守口径。残留 `memset` 还包含 ext4、缓冲区和其他调用者，并不全是 fresh-frame 清零。因此，本轮新清零路径的真实 CPU 宽度不会高于表中的 199/104。

`CowFaultPlan::execute -> alloc_referenced_frame -> alloc_frame -> memset` 的旧路径从预构建 20、正式 8 个样本降为 0；新路径落在 `zero_fresh_frame`。这提供了局部因果证据，足以保留本轮修改。

## 为什么选择这一热点

原子 usage 迭代后，地址空间分配路径仍反复落在：

```text
page fault
  -> CowFaultPlan::execute
    -> alloc_referenced_frame
      -> alloc_frame
        -> memset
```

这里处理的是新分配、尚未安装到用户页表的匿名页或 COW 私有页。内核必须在发布前清零，否则用户态可能读到前一个使用者留下的数据。

原实现对每个字节执行通用 `write_bytes`。RISC-V QEMU `virt` CPU 明确提供 Zicboz，允许用一条 `cbo.zero` 清零一个 cache block。当前 QEMU 的 `riscv,cboz-block-size` 为 64 字节，所以一个 4 KiB 页面需要 64 次 cache-block zero，而不是让通用字节 `memset` 完成整页。

## 为什么不再修改全局 memset

此前尝试把全局 `memset` 改成宽存储，影响了：

- ext4 元数据和数据缓冲；
- 页表；
- 共享内存；
- 任意对齐、任意长度的内核调用者。

全新官方镜像运行出现破坏，因此已经被证伪，详见：

- [全局宽存储 memset 失败报告](../riscv-buildstorm-memset-word-20260725/memset-word-failure-report.md)

本轮不替换全局符号，也不修改 ext4。新函数只由 `alloc_frame(zeroed=true)` 调用，并且清零发生在：

1. 物理 frame 分配成功之后；
2. refcount 注册之前；
3. PTE 安装之前；
4. `Arc` 或 shared backend 发布之前。

因此其他 CPU 和用户态都不可能同时观察这块 frame，清零过程不需要额外共享锁。

## Linux 与硬件口径

Linux RISC-V 的 `arch/riscv/lib/clear_page.S` 采用同一原则：

```text
检测 Zicboz
读取 riscv_cboz_block_size
循环执行 CBO_ZERO
不支持时回退到 __memset
```

Linux 还根据 block order 展开循环。Moss 当前先使用简单循环，避免在没有第二轮证据前扩大汇编实现。

本机 QEMU 10.2.1 生成的 RISC-V `virt` 设备树确认每个 hart 都提供：

```text
riscv,isa-extensions = ..., zicboz, ...
riscv,cboz-block-size = 64
```

启动日志中的 OpenSBI 也列出 `zicboz`。但是内核不能只相信启动日志，所以实现仍在运行时读取设备树，并校验所有 CPU。

## 实现

代码位于 `kernel/src/mm/aspace/backend/mod.rs`。

### 1. 一次性解析设备树

`cboz_block_size()` 使用：

```rust
static CBOZ_BLOCK_SIZE: Once<Option<usize>> = Once::new();
```

第一次 fresh-frame 清零时扫描 CPU 节点，之后直接读取缓存结果。这样不会把 FDT 遍历放进每次缺页。

返回 `Some(block_size)` 必须同时满足：

- 至少存在一个 CPU 节点；
- 每个 CPU 都列出 `zicboz`；
- 每个 CPU 都有 `riscv,cboz-block-size`；
- 所有 CPU 的 block size 相同；
- block size 是 2 的幂；
- block size 不大于 4 KiB。

任何缺失、异构或异常配置都返回 `None`。

### 2. 发布前清零

`zero_fresh_frame(frame, size)` 先把物理地址转换到内核 direct map。只有起始地址和长度都按 block size 对齐时才进入 Zicboz 循环：

```rust
while addr < end {
    cbo.zero(addr);
    addr += block_size;
}
```

当前工具链的内核 target 仍是 `rv64gc`，没有把 Zicboz 变成所有指令都可无条件使用的编译目标。因此内联汇编使用 RISC-V 通用 `.insn` 编码：

```text
opcode = 0x0f
funct3 = 2
rd = x0
imm = 4
```

生成内核反汇编确认循环内存在指令字：

```text
0045200f
```

它与 RISC-V `CBO_ZERO` 的 I-type 编码一致。

### 3. 安全回退

以下情况继续调用原来的 byte-accurate `write_bytes`：

- 非 RISC-V 架构；
- 没有设备树；
- 任一 CPU 不支持 Zicboz；
- block size 缺失或不一致；
- block size 非 2 的幂或大于页面；
- frame 地址或长度不对齐。

LoongArch 因 `cfg(target_arch = "riscv64")` 不会编译 FDT/Zicboz 分支，行为保持原样。

### 4. 内存可见性

`cbo.zero` 修改 cache block 的内存内容。这里不额外加 fence，原因不是“清零可以乱序”，而是发布顺序已经由后续页表/refcount/锁路径保证：

```text
分配 frame
清零 frame
构造 fault 结果
重新取得地址空间锁
验证 fault 仍有效
安装 PTE
```

清零完成之前没有任何可被其他线程使用的 PTE。commit 阶段既不会把未完成清零的 frame 提前发布，也没有并发设备读取该匿名页。

## 三遍静态审查

### 第一遍：调用范围和边界

逐项确认：

- `zero_fresh_frame` 只替换 `alloc_frame` 的原有清零点；
- `zeroed=false` 行为未变；
- 4 KiB PCP 分配和高阶连续分配都使用准确的 `page_size`；
- direct-map 地址来自已成功分配的 frame；
- `checked_add` 阻止区间溢出；
- 循环终止条件与对齐校验覆盖完整范围；
- 不触碰 ext4、page cache、页表和已发布共享页。

### 第二遍：并发、ISA 与回退

逐项确认：

- `Once<Option<usize>>` 并发初始化安全；
- 不会在部分 CPU 支持、部分 CPU 不支持时执行 Zicboz；
- 不会在不同 CPU block size 不一致时执行；
- `.insn` 编码与 RISC-V Zicboz 定义一致；
- `rd=x0`，只有地址寄存器输入；
- `options(nostack)` 不承诺错误的 `nomem` 或 `readonly`；
- frame 尚未发布，因此不需要为清零引入全局锁；
- 非 RISC-V 编译路径只保留原行为。

### 第三遍：机械和构建前完整性

以下检查全部通过：

- `rustfmt --edition 2024 --check kernel/src/mm/aspace/backend/mod.rs`
- `git diff --check`
- README 未修改；
- 未增加依赖；
- 本轮只修改 kernel 源码，不需要更新 vendor checksum；
- 反汇编确认目标指令实际进入 `kernel-rv`，不是被优化器删除；
- 构建日志确认重新编译 `starry-kernel`。

三遍审查全部完成后才开始构建和完整测试。

## 构建与镜像

| 项目 | 值 |
| --- | --- |
| 构建命令 | `make PROFILE=y kernel-rv` |
| 构建耗时 | 12.80 s |
| `kernel-rv` SHA256 | `ac80c849ae2129cd5bba2ccfb5de9da0a55c83b39e987bdfac933c3dfdc2368e` |
| `kernel-rv.profile.elf` SHA256 | `15eb35dd4e84b756b128de83ed0652ac74ee9dc115429eabbe2d145f941945ab` |

本轮从 `/home/wuxun/Downloads/sdcard-rv-pub.img.gz` 重新解压独立镜像：

| 项目 | 值 |
| --- | --- |
| 解压路径 | `tmp/profile-rv-zicboz-zero-20260725/sdcard-rv.img` |
| 解压大小 | 15,032,385,536 字节 |
| SHA256 | `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0` |
| QEMU 写入方式 | `-snapshot` |

镜像哈希也保存于 [disk.sha256](disk.sha256)。`-snapshot` 保证本轮 guest 写入不会回写该 raw 镜像。

## 完整测试口径

QEMU：

```text
qemu-system-riscv64
-machine virt
-m 8G
-smp 8
-bios default
-snapshot
```

profiler 与基线口径完全相同：

| 类型 | 口径 |
| --- | --- |
| CPU | 10 Hz |
| off-CPU | 1/8 抽样，记录权重乘 8 |
| ext4 | 1/16 抽样，记录权重乘 16 |
| page cache | 1/32 抽样，记录权重乘 32 |
| mprotect | 1/32 抽样，记录权重乘 32 |
| 地址空间写锁持有 | 1/16 抽样，记录权重乘 16 |

运行时间：

| 项目 | 值 |
| --- | --- |
| 开始 | 2026-07-25 21:48:07 +08:00 |
| 结束 | 2026-07-25 22:16:38 +08:00 |
| 宿主全流程 | 1711.345 s |
| profile 导出 | GDB 第 1 次成功 |

完整性：

| 项目 | 值 |
| --- | ---: |
| setup phase | 3.668 s |
| prebuild phase | 1118.045 s |
| timed phase | 572.289 s |
| CPU 样本 | 52,485 |
| `dropped_cpu` | 0 |
| `dropped_wait` | 0 |
| `dropped_pending` | 0 |
| `skipped_cpu` | 0 |
| SVG | 112 张 |

cagent 10/10 通过。BuildStorm 正式编译成功，输出 1,681,000 字节内核。`BUILDSTORM_MINIBUILD fail` 是已有脚本前置小测试状态，后续 tg-xtask 预构建与正式 BuildStorm 均成功，不影响本轮完整采样。

## 历次时间总表

| 指标 | 水位总页数 | Cow 内联槽 | bitmap next-fit | 原子 usage | Zicboz 清零 |
| --- | ---: | ---: | ---: | ---: | ---: |
| setup phase | 3.962 s | 3.746 s | 3.908 s | 3.794 s | **3.668 s** |
| prebuild phase | 1161.134 s | 1166.185 s | 1143.762 s | 1137.340 s | **1118.045 s** |
| Cargo 预构建 | 19m20s | 19m25s | 19m02s | 18m56s | **18m37s** |
| timed phase | 580.015 s | 583.185 s | 579.563 s | 574.776 s | **572.289 s** |
| Cargo release | 8m15s | 8m20s | 8m16s | 8m14s | **8m13s** |
| BuildStorm | 510.37 s | 515.89 s | 511.76 s | 509.12 s | **507.87 s** |
| 宿主全流程 | 1756.302 s | 1771.332 s | 1741.349 s | 1726.348 s | **1711.345 s** |

表中连续几轮总体向下，但单次 QEMU 测量不能给出置信区间。只有目标调用栈随代码修改发生大幅、方向一致的变化，才能用于局部归因。

## CPU 火焰图分析

### 新分配页面清零

| 阶段 | 旧 `memset` | 新残留 `memset` | 新 `zero_fresh_frame` | 新保守合计 | 保守下降 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 预构建 | 643 | 129 | 70 | 199 | 444（69.1%） |
| 正式 | 264 | 75 | 29 | 104 | 160（60.6%） |

不能把新版本的 129/75 个 `memset` 全算到 fresh-frame。火焰图中它还包含 ext4 和其他通用清零。采用总和反而高估新实现成本，所以“下降 69.1%/60.6%”是保守下界。

### 非 idle CPU

| 阶段 | 原子 usage | Zicboz 清零 | 变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 8255 | 7713 | -542（-6.57%） |
| 正式 | 3777 | 3607 | -170（-4.50%） |

CPU 样本总数受阶段时间影响，非 idle 变化不能全部归因于清零。但它没有出现“清零路径变窄、其他内核 CPU 同量膨胀”的反向信号。

### 当前正式阶段主要可见路径

| 路径 | 样本 | 判断 |
| --- | ---: | --- |
| ext4 `crc32` | 277 | 当前最宽的具体内核计算路径；已有 unroll8，继续展开需要新证据 |
| `NoPreemptIrqSave::release` | 268 | 多种锁释放的合计，需要继续按直接父调用者拆分 |
| `CurrentTask` | 267 | 既有 direct-current 迭代未获得端点收益 |
| `UserContext::run` | 150 | 用户态执行采样边界，不等于该函数自身消耗 150 个样本 |
| BTree 查询 | 约 95 | 来自 COW/file page 结构，已有 HashMap 失败记录 |
| `zero_fresh_frame` | 29 | 本轮目标已经从首要热点降为次要路径 |

下一轮不应再继续优化 fresh-frame 清零。当前剩余路径需要先拆出调用者，尤其不能把所有 `release` 误认为同一把锁。

## 文件系统 I/O 与等待

| 事件 | 基线预构建 | Zicboz 预构建 | 变化 | 基线正式 | Zicboz 正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 同步文件系统 I/O | 128.876 s | 127.311 s | -1.565 s | 76.967 s | 69.646 s | -7.321 s |
| ext4 锁等待 | 3.599 s | 3.385 s | -0.214 s | 1.213 s | 0.892 s | -0.321 s |
| folio 加载等待 | 2.889 s | 3.099 s | +0.210 s | 0.379 s | 0.342 s | -0.037 s |

预构建同步 I/O 基本持平，说明没有少执行一大段工作。正式 I/O 比基线低 7.321 秒，它可能解释部分端点改善，但 fresh-frame 清零不会直接改变 ext4 I/O 算法，因此这部分应视为运行波动或间接调度变化，不能计入 Zicboz 的确定收益。

## 地址空间写锁累计持有

这些数值是抽样加权后的累计持有时间，多核同时持有不同地址空间的锁时会重叠，不能与墙钟时间直接相加。

| 事件 | 基线预构建 | Zicboz 预构建 | 基线正式 | Zicboz 正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child | 48.658 s | 52.961 s | 21.866 s | 20.355 s |
| clone parent | 24.922 s | 29.170 s | 11.476 s | 9.742 s |
| exit clear | 50.583 s | 50.667 s | 21.762 s | 21.439 s |
| exec clear | 31.356 s | 27.431 s | 10.404 s | 9.660 s |
| mprotect | 48.490 s | 48.448 s | 21.215 s | 21.001 s |
| munmap | 26.829 s | 24.845 s | 9.965 s | 8.772 s |

变化有升有降，mprotect 和 exit 基本稳定。它排除了“地址空间操作数量突然整体降低”这一解释。Zicboz 修改也没有扩大地址空间写锁范围。

mprotect 调用量保持同一量级：

| 阶段 | 调用次数 |
| --- | ---: |
| 预构建 | 1,465,614 |
| 正式 | 668,249 |

## 火焰图与原始数据

- [完整 CPU](guest-cpu-all.svg)
- [预构建 CPU](guest-cpu-prebuild.svg)
- [正式 CPU](guest-cpu-timed.svg)
- [完整 off-CPU](guest-offcpu-all.svg)
- [预构建 off-CPU](guest-offcpu-prebuild.svg)
- [正式 off-CPU](guest-offcpu-timed.svg)
- [正式锁等待](guest-offcpu-lock-timed.svg)
- [正式同步文件系统 I/O](guest-fs-io-latency-timed.svg)
- [正式 ext4 锁等待](guest-ext4-lock-wait-timed.svg)
- [正式 folio 加载等待](guest-folio-load-wait-timed.svg)
- [正式 clone child 写锁](guest-aspace-clone-child-write-hold-timed.svg)
- [正式 clone parent 写锁](guest-aspace-clone-parent-write-hold-timed.svg)
- [正式 exit clear 写锁](guest-aspace-exit-clear-write-hold-timed.svg)
- [正式 exec clear 写锁](guest-aspace-exec-clear-write-hold-timed.svg)
- [正式 mprotect 写锁](guest-aspace-mprotect-write-hold-timed.svg)
- [正式 munmap 写锁](guest-aspace-munmap-write-hold-timed.svg)
- [原始运行元数据](run.json)
- [渲染摘要](summary.json)
- [符号化 profile 日志](profile.log)

## 本轮决策

保留本轮修改，理由按证据强弱排序：

1. 目标 fresh-frame `memset` 调用栈消失；
2. 保守合并清零 CPU 宽度在两个阶段分别下降 69.1% 和 60.6%；
3. 非 idle CPU 样本同时下降，没有出现等量转移；
4. 完整 cagent 和 BuildStorm 成功；
5. 新鲜官方镜像哈希一致，`-snapshot` 没有污染基线；
6. 端点时间方向一致，但只作为辅助证据。

下一轮先对正式阶段剩余 268 个 `NoPreemptIrqSave::release` 样本按直接父路径重新归类，并结合锁等待图确认是否仍有一把高频、可隔离的锁。只有静态证明不会扩大临界区或把并行路径重新串行化后，才进行下一次修改和三遍审查。
