# RISC-V lwext4 CRC32 八次展开迭代报告

## 结论

本轮保留 CRC32 的八次循环展开，但不能宣称端到端编译已经变快。

局部证据明确支持 CRC 热点下降：

- 正式阶段 `crc32` 叶样本从 `337 / 4304 = 7.83%` 降到
  `292 / 4931 = 5.92%`；
- 相对非 idle CPU 占比下降 24.4%；
- `task:sync` 中的 CRC 样本从 310 降到 276，尽管本轮正式 phase
  反而长 14.1%；
- `timed phase - BUILDSTORM_COMPILE` 从 75.84 秒降到 72.74 秒，
  该区间包含 BuildStorm 外围和结束后的同步/采样收尾，单轮少 3.10 秒。

但是，本轮完整 BuildStorm 为 594.28 秒，接受基线为 508.98 秒，慢
85.30 秒，即 16.76%。预构建也从 19m09s 变为 22m15s，慢 16.23%。
多个与 CRC 无关的事件持有时间同步放大约 15%–22%，而工作负载调用数几乎
不变，说明本轮存在全程吞吐漂移。不能把这 85 秒反向归因于 CRC，也不能
用局部样本下降掩盖端点结果。

准确判断是：

1. CRC 展开在 CPU 热点上有效，语义和完整功能测试均通过；
2. 本轮环境没有提供可比较的绝对端到端时间；
3. 修改只改变循环结构，不增加锁、全局状态或并发语义，可以保留到下一轮
   继续交叉验证；
4. 下一轮完整运行仍必须同时报告绝对端点；如果环境恢复，才能观察组合
   结果是否回到原来的 509 秒附近。

## 修改内容

原实现每处理一个字节都执行一次循环判断和跳转：

```c
while (size--)
	crc = tab[(crc ^ *p++) & 0xFF] ^ (crc >> 8);
```

本轮增加一个 `size >= 8` 主循环，在一次循环中连续执行八个完全相同的
逐字节更新，最后继续使用原循环处理 0–7 字节尾部。

没有做以下修改：

- 没有改 CRC32 或 CRC32C 多项式和查找表；
- 没有宽读输入缓冲区；
- 没有改变每个字节参与 CRC 的顺序；
- 没有运行时初始化表；
- 没有锁、原子变量或共享可变状态；
- 没有使用目标机不具备的 RISC-V Zbc 扩展。

## 为什么前一项 `memset` 优化被撤销

上一候选把内核最终链接到的全局弱 `memset` 改为 8 字节存储。独立
RISC-V 边界测试覆盖 786,624 个用例且全部正确，但两个全新官方镜像分别
复现：

1. `ring` 对象文件变为非法格式、同一路径随后成为目录；
2. `PageTable64::drop` 读取已失效页表页并触发 Supervisor Page Fault。

因此它属于顺序函数正确、系统并发生命周期不安全的修改，已经完全撤销。
详细证据见
`../riscv-buildstorm-memset-word-20260725/memset-word-failure-report.md`。

## 三遍静态审查

### 第一遍：算法和边界

- `size=0..7` 只进入原尾循环；
- `size=8` 恰好执行一次展开循环；
- 非 8 倍数先处理完整八字节组，再处理余数；
- `size` 为 `uint32_t`，只有在 `size >= 8` 时才减 8，不会下溢；
- 每个更新都读取一个字节并立即更新下一步依赖的 `crc`；
- 20,000 组随机初值、随机 256 项表和随机 0–256 字节输入与旧模型逐项
  相等。

### 第二遍：C 语义和架构

- `crc` 是 `uint32_t`，右移保持逻辑右移；
- 表元素和异或结果都是 32 位无符号值；
- 每条表达式只有一个 `p++`，不存在未排序的重复修改；
- 输入仍按 `uint8_t` 读取，不产生未对齐宽访问；
- RISC-V 和 LoongArch 交叉编译器都通过
  `-Wall -Wextra -Wshadow -Werror -fsyntax-only`。

### 第三遍：仓库和构建输入

- `git diff --check` 通过；
- `cargo fmt --all --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- 本轮运行代码只修改 `ext4_crc32.c`；
- `third_party/lwext4_rust` 不使用 vendor checksum；
- `build.rs` 对整个 `c/lwext4/src` 声明了重新构建依赖；
- 三遍检查完成后才清理 `lwext4_rust` 的 RISC-V release 产物。

## 构建和机器码验证

构建命令：

```text
cargo clean -p lwext4_rust --target riscv64gc-unknown-none-elf --release
make PROFILE=y kernel-rv
```

构建成功，用时 12.15 秒。反汇编确认：

- 新的共享 `crc32` 主循环一次读取偏移 0–7 的八个字节；
- 八次表查询保持串行依赖；
- 输入指针每轮增加 8；
- 主循环只有一次回跳；
- `size & 7` 后仍进入逐字节尾循环；
- `ext4_crc32c` 只是装入 CRC32C 表地址后跳入共享实现。

代码尺寸从原先约 60 字节的 `ext4_crc32c` 增加为 306 字节共享主体加
20 字节包装。这个增量需要关注 I-cache，但不足以单独解释所有无关路径
同时约 15% 的吞吐漂移。

## 完整测试口径

- Ubuntu 宿主直接运行，不使用 Docker；
- RISC-V `virt`；
- 8 GiB 内存、8 vCPU；
- QEMU `-snapshot`；
- 根文件系统从官方压缩包重新复制、解压；
- 镜像大小：15,032,385,536 字节；
- 镜像 SHA-256：
  `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0`；
- CPU 10 Hz；
- off-CPU 1/8；
- ext4 1/16；
- page cache 1/32；
- AddrSpace hold 1/16；
- mprotect 1/32。

采样完整：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
SVG=112
```

cagent 10/10 通过，预构建和正式 BuildStorm 均成功，正式产物
1,681,000 字节。

## 时间对比

| 指标 | guard-inline 接受基线 | CRC 八次展开 | 变化 |
| --- | ---: | ---: | ---: |
| setup phase | 4.25 s | 4.31 s | +0.06 s |
| 预构建 phase | 1150.13 s | 1336.95 s | +186.83 s / +16.24% |
| Cargo 预构建自报 | 19m09s | 22m15s | +3m06s / +16.23% |
| timed phase | 584.82 s | 667.02 s | +82.20 s / +14.05% |
| BuildStorm 自报 | 508.98 s | 594.28 s | +85.30 s / +16.76% |
| Cargo release 自报 | 8m13s | 9m37s | +1m24s / +17.04% |
| timed - BuildStorm | 75.84 s | 72.74 s | -3.10 s / -4.09% |
| 宿主全流程 | 1756.23 s | 2026.30 s | +270.07 s / +15.38% |

预构建、正式 Cargo 和宿主全流程都以接近的比例变慢，这不是只在正式
`sync` 中占大头的 CRC 能解释的形态。

## CPU 火焰图对比

### 正式阶段

| 叶路径 | 基线 | 本轮 | 占比变化 |
| --- | ---: | ---: | ---: |
| `crc32` 主体 | 337 / 7.83% | 292 / 5.92% | -24.4% |
| `CurrentTask::from_raw_ptr` | 276 / 6.41% | 353 / 7.16% | +11.7% |
| `memset` | 238 / 5.53% | 268 / 5.44% | -1.6% |
| `UserContext::run` | 210 / 4.88% | 262 / 5.31% | +8.8% |
| BTree `find_key_index` | 109 / 2.53% | 144 / 2.92% | +15.4% |
| concurrent page fault | 103 / 2.39% | 109 / 2.21% | -7.5% |

正式阶段非 idle 总样本从 4304 增到 4931，与 phase 增长一致。
`task:sync` 的 CRC 样本却从 310 降到 276，是局部优化成立的最直接证据。

### 预构建

预构建 CRC 原本只占 0.84%，不是主要成本。本轮为 100 个相关样本，占
0.88%；这个阶段不能用来证明 CRC 收益。它的 16.24% 增时主要来自整体
执行吞吐变化和最后的 `axbuild/tg-xtask` 依赖尾部。

## 为什么端到端差值不能归因于 CRC

两轮工作负载规模几乎相同：

- 基线正式 mprotect 调用：665,554；
- 本轮正式 mprotect 调用：664,920；
- 差异：-0.095%。

但与 CRC 无关的累计持有时间普遍增加：

| 事件 | 基线正式 | 本轮正式 | 变化 |
| --- | ---: | ---: | ---: |
| exit clear 写锁 | 25.25 s | 31.81 s | +25.99% |
| mprotect 写锁 | 21.58 s | 25.23 s | +16.87% |
| exec clear 写锁 | 12.10 s | 17.37 s | +43.55% |
| munmap 写锁 | 11.03 s | 15.36 s | +39.20% |

这些是所有任务的累计时间，不是可直接相加的墙钟时间；但它们和 Cargo
端点一起表明整轮执行速度发生了漂移，而不是 CRC 多做了 85 秒工作。

## 下一热点选择

本轮之后，正式 CPU 图中最宽的叶路径依次是：

1. `NoPreemptIrqSave::release`：9.23%，受 IRQ 恢复后的延迟定时器采样偏置，
   不能按宽度直接优化；
2. `CurrentTask::from_raw_ptr`：7.16%，此前 direct-current 迭代已经证明
   单独去包装没有端到端收益，进一步传递 pending token 的尝试还触发过
   页表生命周期崩溃；
3. `crc32`：5.92%，本轮已降低，不宜继续只增加展开代码；
4. `memset`：5.44%，全局宽存储优化已被两次全新镜像证明不安全；
5. `UserContext::run`：5.31%，属于用户/内核切换采样落点，不等同于函数
   自身消耗；
6. `Rv64PTE::is_huge`：2.01%，是简单、频繁、无状态的只读页表访问器。

所以下一轮选择给 RISC-V PTE 的只读基础访问器增加普通 `#[inline]`，
重点验证 `is_huge` 是否从独立叶热点中消失。它只改变调用边界，不改变页表
位定义、原子操作、锁和生命周期，风险显著低于继续修改 preempt 或全局
`memset`。

## 火焰图

- [完整 CPU](guest-cpu-all.svg)
- [预构建 CPU](guest-cpu-prebuild.svg)
- [正式 CPU](guest-cpu-timed.svg)
- [完整 off-CPU](guest-offcpu-all.svg)
- [工作负载 off-CPU](guest-offcpu-workload-all.svg)
- [锁等待 off-CPU](guest-offcpu-lock-all.svg)
- [ext4 全局锁等待](guest-ext4-lock-wait-all.svg)
- [folio 加载等待](guest-folio-load-wait-all.svg)
- [同步文件系统 I/O](guest-fs-io-latency-all.svg)
- [mprotect 地址空间锁等待](guest-mprotect-aspace-lock-wait-all.svg)
- [exit clear 写锁](guest-aspace-exit-clear-write-hold-all.svg)
- [exec clear 写锁](guest-aspace-exec-clear-write-hold-all.svg)
- [clone child 写锁](guest-aspace-clone-child-write-hold-all.svg)
- [clone parent 写锁](guest-aspace-clone-parent-write-hold-all.svg)
