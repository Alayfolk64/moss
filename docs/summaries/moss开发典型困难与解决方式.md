# MOSS 开发过程中典型的困难与解决方式

本文根据本机当前保留的 MOSS 会话归档整理，覆盖 2026 年 7 月 27 日至 8 月 14 日，并过滤了审批代理、分叉会话和重复上下文。下面不是普通操作失误清单，而是整个开发过程中真正定位困难、具有复用价值的问题。

## 一、内核并发与性能优化

### 1. 缺页处理长期持有 AddrSpace 独占锁

- 现象：BuildStorm 多线程编译时 CPU 利用率不高，大量线程阻塞；一个线程等待 ext4/磁盘 I/O 时，其他线程的缺页、`mmap`、`mprotect` 都被堵住。
- 根因：旧链路是“拿地址空间写锁 → 查 VMA → 查页缓存 → 等磁盘 → 分配页 → 安装 PTE → 解锁”，把慢 I/O 包在了进程级大锁里。
- 解决：改成 `prepare → execute → commit` 三阶段。锁内只记录 VMA、权限和版本，锁外完成页缓存及磁盘 I/O，最后短暂重新加锁，确认状态未变化后安装 PTE；发生竞态时有限重试，再退回保守路径。
- 经验：锁优化不能只看“锁持有时间”，还必须设计状态版本、失败回滚和竞态重试。

### 2. 三阶段缺页修好性能后，又破坏了嵌套 fork

- 现象：内核能启动，但 shell → timeout → agent/cargo 连续 fork 后出现空地址跳转，cagent 和 minibuild 崩溃。
- 根因：给子地址空间创建了新的 COW backend identity，却没有统计已经复制到子页表中的驻留页。下一次嵌套 fork 错误地认为地址空间完全懒加载，漏掉了 COW 页。
- 解决：复制驻留映射时同步维护 `mapped_pages`，失败回滚时也恢复计数。
- 经验：COW 优化不只是复制 PTE，还要同步维护引用、驻留页计数、backend 身份和回滚归属。

### 3. COW 快路径在关中断状态发起同步跨核 TLB 刷新

- 现象：LoongArch 12 核启动时在 IPI 断言处 panic。
- 根因：独占 COW 页优化持有 `SpinNoIrq`，当前 CPU 中断已经关闭；此时修改 PTE 后又同步通知其他 CPU 刷新 TLB。同步 IPI 要求发起核能处理中断，否则可能形成跨核互等。
- 解决：同一物理页仅从只读升级为可写时只刷新本核；确需跨核 shootdown 的操作必须在释放关中断锁后执行。
- 经验：页表更新正确不代表刷新方式正确。必须同时审计锁、IRQ、IPI 和 TLB 的调用契约。

### 4. 单独移植 COW fork 优化导致 BuildStorm 全核 idle

- 现象：编译中途无日志，12 个虚拟 CPU 全部进入 `run_idle`，QEMU 永不退出。
- 根因：子进程不再继承已驻留的只读文件 folio PTE，而是在首次访问时重新缺页并进入异步页缓存等待；当前 `main` 缺少原优化分支中的配套调度和唤醒条件，偶发丢失唤醒。
- 解决：保留稀疏页表遍历，但继续向子进程继承已经驻留的文件 folio PTE、folio 引用和 `mapped_pages`。
- 经验：一个优化在原分支可用，不代表复制核心代码后仍成立；必须同时迁移它依赖的状态维护和唤醒语义。

### 5. `vfork` 被退化成普通 COW fork

- 现象：Cargo/rustc 大量启动短生命周期进程时性能明显下降，外部命令类 cagent 用例也不稳定。
- 根因：代码看到 `CLONE_VFORK` 后反而删除了 `CLONE_VM`，父进程也不等待子进程 `exec/exit`，使轻量 `vfork+exec` 变成完整地址空间克隆。
- 解决：恢复 `VforkCompletion`：父进程等待；子进程成功 `execve` 或退出时唤醒父进程。
- 同时发现的文件系统回退：
  - 路径解析期间长期持有 `FS_CONTEXT` 大锁；
  - 每次关闭可写文件都强制 `sync`；
  - 查询目录元数据时遍历整个目录计算 `nlink`。
- 经验：BuildStorm 的热点不仅是编译器本身，更包括 `vfork/exec/wait/open/stat/close` 这些高频系统调用链。

### 6. PCP 物理页缓存“理论优化”一度反而变慢

- 现象：引入 per-CPU page cache 后，某些实验成绩下降。
- 根因：PCP 命中快路径使用了过重的 `SpinNoIrq`，每次单页分配都额外维护抢占状态；实验中还混入了其他修改，难以单独归因。
- 解决：快路径采用更轻的 IRQ guard，慢路径才显式关闭抢占；使用批量 refill/drain，并在 OOM 前回收各 CPU 缓存。
- 经验：per-CPU 不天然等于快，关键是命中路径到底执行了多少同步操作。

## 二、构建、实验与跨平台问题

### 7. 源码明明有接口，编译器却说接口不存在

- 现象：出现 `disable_preempt_irq_disabled`、`write_unpublished` 等错误，但当前源码中的 trait 或方法明明已经修改正确。
- 根因：交叉编译目录仍在使用旧的 vendored crate 元数据；`cargo clean -p` 因 source-id 不同删除了 0 个文件。另一些平台包按不可变 registry vendor 使用，直接改源码也不会触发重编译。
- 解决：清理整个 target；需要持续修改的平台 crate 改成本地 path dependency。
- 经验：遇到“编译器看到的接口与打开的源码不一致”，优先验证依赖解析结果和缓存，不要立刻补一个错误的兼容实现。

### 8. 实验数据无法形成可靠的逐阶段对比

- 现象：计划比较 baseline 和六项优化，但历史提交把多项优化混在两个大提交里，有些结果还受缓存、镜像污染或失败运行影响。
- 解决：
  - 建独立实验分支；
  - 一个优化一个累计提交；
  - 每次使用同一干净 rootfs 和 QEMU snapshot；
  - 记录代码 SHA、内核 SHA、rootfs SHA、完整 QEMU 参数和原始日志；
  - 至少重复三次取中位数；
  - 只有出现正式完成标志并正常退出才采纳数据。
- 经验：性能实验最难的常常不是优化本身，而是证明“只改变了一个变量”。

### 9. 内核编译配置与 QEMU 虚拟硬件配置不一致

- 现象：QEMU 使用 `-m 36G -smp 12`，但内核可能按 `8G/8核` 编译；本机又因只有约 14 GiB 内存而无法启动 36G 客体。
- 根因：Makefile 的 `MEM/SMP` 决定内核编译期容量，QEMU 的 `-m/-smp` 决定虚拟硬件，两者互不覆盖。
- 解决：提交测评前让两边严格一致；本机资源不足时，用低内存同核数做功能验证，但不能把它当成完整测评结果。
- 经验：QEMU 必须先成功分配 `-m` 指定的内存，不能简单理解为“编译值和 QEMU 值取较小者”。

### 10. CPU 数量正确，但 CPU 编号映射错误

这个问题先后出现了三种形式：

- OpenSBI 随机选择 hart 4/6 启动，内核却把 DTB 中的 hart 0 固定当作逻辑 CPU 0。
- QEMU 10.2 用 `mmu-type = "riscv,sv57"` 表示 S-mode，旧代码只在 `riscv,isa` 中找 `s`，结果过滤掉全部 CPU。
- IPI、远程 TLB 和 PLIC context 把 MOSS 逻辑 CPU ID 直接当成硬件 hart ID。

解决方式是：

- 从真实内核入口记录启动硬件 ID；
- 将实际启动核排到逻辑槽 0；
- 同时识别 `mmu-type`；
- 在 HAL 硬件边界统一完成“逻辑 ID → 硬件 ID”转换；
- VF2 还要排除不能运行 MOSS 的 S7/hart 0，而不能简单把容量从 4 改成 5。

## 三、2K1000LA 实板适配

### 11. 厂商 U-Boot 自带设备树与 MOSS 驱动不匹配

- 现象：使用 `fdtcontroladdr` 中的设备树时，`/2k1000-soc/ahci@400e0000` 等节点不存在，MOSS 无法按预期初始化 AHCI、网卡和 rootfs。
- 根因：U-Boot 内部控制设备树主要服务于固件自身，节点结构和属性与当前 MOSS 2K1000LA 平台驱动期望不一致。
- 解决：使用经过验证的 `board.dtb`，放到电脑端 `/srv/tftp/board.dtb`，由 U-Boot 单独 TFTP 到内存；设置 `/chosen/bootargs`、网卡 alias 和 AHCI 属性后，再加载 MOSS 内核启动。
- 经验：内核不会自动从电脑或硬盘“扫描到 DTB”。设备树由 bootloader 准备，内核得到的是一个内存地址。

相关操作已经整理在 [2K1000LA板-启动指令.md](../guides/boards/2K1000LA板-启动指令.md)。

### 12. `tar` 在 2K1000LA 上触发 `AddressNotAligned/SIGBUS`

- 现象：glibc/cagent 压缩包 SHA 正确，但 `tar -tzf`、`tar -xzf` 在 BusyBox `memcpy` 中触发非对齐异常和 `SIGBUS`。
- 根因：故障指令是索引式 `stx.d`，板上 BADV CSR 给出的地址并不等于寄存器实际计算的访问地址；模拟器继续信任 BADV，修复第一次仍失败。
- 解决：根据陷阱现场的指令和寄存器重新计算有效地址，并覆盖索引式、立即数式及 `ldptr/stptr` 等访存格式。
- 经验：异常寄存器也可能不可靠。必须把故障指令、寄存器和地址三者交叉验证。

同一阶段还发现 AHCI `flush()` 原来直接返回成功。后来补上了真实 ATA `FLUSH CACHE/FLUSH CACHE EXT` 和 Task File Error 检查，避免 `sync` 表面成功但 SSD 尚未持久化。

## 四、VisionFive 2 与文件系统

### 13. 仓库里有 `make vf2`，不等于 VF2 平台真的存在

- 现象：Makefile、feature 和 `extern crate` 都保留了 VF2 入口，但构建提示缺少 `axplat-riscv64-visionfive2/axconfig.toml`。
- 根因：早期依赖降级/离线化时删除了真正的平台 crate，却没有清理入口；旧平台包接口版本也与当前 MOSS 不兼容。
- 解决：按当前 `axplat/axcpu` 接口重新接回启动、UART、timer、PLIC、SMP、SD 和 GMAC，而不是简单复制旧 crate。
- 经验：判断平台支持程度要从“能否生成镜像并上板”开始，不能只搜索 target 名字。

### 14. VF2 SD 卡同时存在可靠性、语义和性能三层问题

先后遇到：

- 高负载读取出现 `CMD17 timeout`，错误路径 `unwrap()` 导致整机 panic；
- FIFO 被当作递增内存地址，并使用错误的 64 位访问，实际应固定地址、32 位宽度；
- `flush()` 曾是空实现；
- 纯 PIO 冷启动 `rustc` 需要一分钟以上，看似卡死；
- 后续 IDMAC DMA 还要处理 32 位地址范围、缓存 clean/invalidate、4 KiB 描述符边界和多块 CMD18/CMD25。

解决过程：

1. 先用有界同步 PIO、控制器恢复、有限重试和错误上报保证正确性。
2. 修正 FIFO 地址与访问宽度。
3. 再给 VF2 单独启用 IDMAC DMA，其他平台保持原路径。
4. 用 4 KiB、64 KiB、768 KiB 和完整 BusyBox 文件做 `sync + SHA256` 实板校验。

经验：驱动应先做到“失败可恢复、错误可见”，再做 DMA；否则 DMA 只会让数据损坏更难定位。

### 15. 文件显示保存成功，重启后追加内容却全是零

- 现象：文件长度正确，原内容正确，但追加部分重启后变成 `0x00`。
- 排查误区：先后怀疑 SD 卡忙状态、空 `flush()`、CMD13 和 FIFO；这些确有缺陷，但都不是最终根因。
- 真正根因：页缓存已经把追加数据写入磁盘，随后 ext4 提交新文件长度时又调用普通扩容语义，把旧 EOF 到新 EOF 区间清零，覆盖了刚写入的数据。
- 解决：区分两种操作：
  - 用户 `truncate/ftruncate` 扩容：新增区域必须补零；
  - 页缓存数据写回后提交长度：只更新 inode 长度，不能再次清零。
- 经验：长度正确不代表数据正确；持久化验证必须包含丢缓存或重启后的字节级比较。

### 16. Git/Vim 暴露目录项和 inode 缓存代际错误

出现过两个非常典型的现象：

- `git commit` 明明成功，紧接着 `git log` 却说没有提交；执行一次 `ls .git/refs/heads` 后又恢复。
- Rust 源文件第一次读取时混入 `.viminfo` 尾部，或者 Vim 报 54B written，但 `rustc` 读到 0 字节、链接数为 0 的旧 inode。

根因包括：

- 同一个目录 inode 存在多个 `DirNode` 包装，各自持有不一致的正/负 dentry 缓存；
- rename 只刷新了其中一个包装；
- ext4 复用 inode 号时，旧文件的页缓存和元数据缓存仍按 `(文件系统, inode号)` 命中。

最终解决：

- 目录项版本细化为“目录 inode + 文件名”；
- create/unlink/link/rename 后精准失效对应名字；
- 跨目录 rename 按 inode 顺序双锁；
- 新建 inode 时清理同号旧页缓存和元数据缓存。

经验：文件系统缓存必须带“代际”概念；inode 号相同不代表还是同一个文件。

## 最值得在汇报中重点讲的五个案例

如果要向老师或评委讲，建议重点选择：

1. 2K1000LA 厂商设备树与 MOSS 驱动不匹配，改用外部 `board.dtb`。
2. AddrSpace 三阶段缺页，把磁盘 I/O 移出进程级大锁。
3. COW 持有关中断锁时发同步 IPI，导致 LoongArch 多核 panic。
4. VF2 文件追加“长度正确、内容归零”，最终定位到 ext4 长度提交二次清零。
5. Git/Vim 暴露 dentry 和 inode 复用缓存代际错误。

这些案例既有硬件适配，也有并发、性能、文件系统和跨架构问题，最能体现 MOSS 开发工作的技术深度。现有细节还可结合 [BuildStorm 优化报告](../reports/buildstorm-report/main.typ)、[VF2 工作总结](星光二号板工作总结.md) 和 [2K1000LA 调试状态](../history/boards/龙芯二号板调试状态报告-20260808.md)继续整理成正式的“问题—定位—修复—验证”章节。
