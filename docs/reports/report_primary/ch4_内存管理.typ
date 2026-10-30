#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(4)

= 内存管理

== 概述

StarryOS 已经具备用户虚拟内存管理的主体框架，其核心结构如 @fig:aspace-struct 所示。`AddrSpace` 记录可用地址范围和已装入的页表快照；`MemoryArea` 按起始地址组织互不重叠的虚拟区域，每个区域绑定一个 `Backend` 决定页面从何处取得；`Backend` 封装四种页面来源。多级页表在缺页时按需装入物理页，`fork` 通过 `AddrSpace::try_clone` 复制区域描述并对私有可写页面执行 COW 保护。

#figure(
  [
    ```
    AddrSpace
    ├── va_range             可用虚拟地址上下界
    ├── areas: MemorySet
    │   └── BTreeMap<VirtAddr, MemoryArea>
    │       ├── va_range     连续的虚拟地址范围
    │       ├── flags        读 / 写 / 执行 / 用户态权限
    │       └── backend      Linear | Cow | Shared | File
    └── pt: PageTable        已装入的虚拟页 → 物理页快照
    ```
  ],
  caption: [AddrSpace 三层结构概览],
) <fig:aspace-struct>

MossOS 沿用这套框架，我们的工作集中在 `mmap` 系统调用的语义补全上。StarryOS 的 `sys_mmap`（`kernel/src/syscall/mm/mmap.rs`）主干已能处理 `MAP_PRIVATE`、`MAP_SHARED` 和 `MAP_ANONYMOUS` 的基本组合，对应关系如 @tab:mmap-backend-base 所示。但若干标志和配套接口仍为占位实现——`sys_mlock` 直接返回成功而未执行实际锁定，`sys_madvise` 忽略 advice 参数，`sys_mremap` 强制以私有匿名映射重建目标区域。MossOS 将这些占位实现补全为完整语义，将 mmap 相关代码从 337 行扩展到 1023 行。

#figure(
  table(
    columns: (1fr, 0.4fr, 1.6fr),
    inset: (x: 10pt, y: 6pt),
    stroke: 0.5pt + gray,
    table.header(
      align(center, [*映射请求*]),
      align(center, [*Backend*]),
      align(center, [*行为*]),
    ),
    [`MAP_PRIVATE  MAP_ANONYMOUS`], [Cow], [建立私有匿名区域，首次访问时清零分配],
    [`MAP_PRIVATE` 文件映射], [Cow], [从文件读取初始内容，写入后仅修改私有副本],
    [`MAP_SHARED  MAP_ANONYMOUS`], [Shared], [分配物理页槽位，多进程共享同一组页面],
    [`MAP_SHARED` 文件映射], [File], [通过文件缓存访问，多映射看到同一份内容],
    [设备物理范围], [Linear], [按固定偏移直接映射设备提供的物理地址],
  ),
  caption: [mmap 标志与 Backend 的对应关系（StarryOS 基线已具备）],
) <tab:mmap-backend-base>

@tab:mmap-additions 总结了 MossOS 在 mmap 上的各项补全及其动机。

#figure(
  table(
    columns: (0.8fr, 1fr, 2.2fr),
    align: (center, left, left),
    inset: 8pt,
    stroke: 0.5pt + gray,
    table.header(
      align(center, [机制]),
      align(center, [基线状态]),
      align(center, [MossOS 的处理]),
    ),
    [栈自动增长], [`MAP_GROWSDOWN` 已定义但忽略], [缺页时自动向下扩展一页，避免大局部变量或深层递归直接触发 SIGSEGV],
    [mremap 迁移], [不处理范围重叠], [支持 `MREMAP_FIXED`，先解除重叠区域再建立新映射，防止数据丢失],
    [WIPEONFORK], [不存在], [fork 后在子进程中清零标记区域，避免父子进程共享 malloc 元数据导致堆损坏],
    [/proc/[pid]/maps], [不存在], [输出 Linux 兼容的映射列表，供 gdb、pmap、ASAN 等工具获取进程内存布局],
    [PROT_WRITE], [W=1,R=0 导致页表映射失败], [自动补 READ 位但保持用户可见 prot 不变，规避 RISC-V 特权规范的可写页必须可读的限制],
    [SIGBUS 追踪], [无], [文件尾外写入触发 SIGBUS 而非 SIGSEGV，区分"地址合法但存储不可用"与"地址非法"],
  ),
  caption: [MossOS 在 mmap 上的语义补全],
) <tab:mmap-additions>

下面逐项展开这些机制。

== 栈自动增长

用户栈在程序启动时通常只分配几页。C 程序中声明大型局部数组、深层递归调用或 alloca 动态分配都可能超出初始栈空间。如果内核在栈耗尽时直接发送 `SIGSEGV`，大量合法程序将无法运行。Linux 的做法是：栈区域以 `MAP_GROWSDOWN` 标记，缺页时自动向下扩展一页，而非拒绝访问。

@fig:growdown-flow 展示了 GROWSDOWN 缺页的处理路径。内核先调用 `AddrSpace::handle_page_fault` 尝试按已注册区域填页；若未命中任何区域，则在返回前调用 `try_growdown_fault` 检查是否有 GROWSDOWN 区域可以扩展。全局 `GROW_DOWN_AREAS` 表以进程 pid 为键，记录每个区域的当前边界。若缺页地址落在某个区域起始地址以下的 4KiB 范围内，则进入 `block_growdown`——为新页面建立物理映射并向下扩展区域边界。

#figure(
  [
    ```
    handle_page_fault(vaddr)
    ├── areas.find(vaddr) 命中 → 正常 Backend::populate
    └── 未命中
        └── try_growdown_fault(vaddr)
            ├── 遍历 GROW_DOWN_AREAS[pid]
            ├── 地址在 [area.low, area.start) 且距 start ≤ 4KiB?
            │   ├── 是 → block_growdown(area)
            │   │       ├── 分配清零页 + 建立页表映射
            │   │       └── 更新 area.start -= PAGE_SIZE
            │   └── 地址触及 area.low → SIGSEGV
            └── 无匹配 → SIGSEGV
    ```
  ],
  caption: [GROWSDOWN 缺页处理流程],
) <fig:growdown-flow>

扩展不是无限制的。每个 GROWSDOWN 区域有一个预设的 `low` 下界（通常为栈底预留的保护页），地址触及下界后不再扩展，转为 `SIGSEGV`。这防止了栈无限增长侵入堆或其他映射区域。扩展后的新页面由 Cow Backend 管理，首次访问时清零分配，`fork` 时通过 COW 与父进程共享。

== mremap 迁移

`mremap` 在原始区域后方空间不足以容纳新大小时，需要将映射迁移到新地址。StarryOS 基线的做法是以 `MAP_PRIVATE | MAP_ANONYMOUS` 在新地址重建映射、复制数据、再解除旧映射。这能完成数据搬迁，但引入了两个实际问题。

第一个问题是新旧地址范围可能部分重叠。如果新地址落在旧地址范围中间，朴素的"先建新映射再删旧映射"会导致新建的区域被后续的 unmap 操作误删。MossOS 改为在建立新映射前先检查是否与旧范围重叠，若重叠则先解除旧映射的重叠部分，再在新地址建立映射。

第二个问题是迁移后原始区域的 Backend 类型信息丢失——文件映射退化为匿名映射，`/proc/[pid]/maps` 不再能显示正确的路径和偏移。MossOS 在迁移后保留原始区域在 `MMAP_RECORDS` 表中的记录，确保 `maps` 输出仍能反映正确的权限和映射类型。迁移路径仍以私有匿名映射重建（MossOS 未改变这一保守策略），但 `MREMAP_MAYMOVE` 未设置且后方空间不足时，正确返回 ENOMEM 而非强行迁移。

== MADV_WIPEONFORK

多线程程序中，`fork` 只复制调用线程，子进程的地址空间是父进程的完整快照。如果父进程在 fork 前通过 `malloc` 分配了内存，子进程继承的堆中包含了父进程 allocator 的内部元数据（如 jemalloc 的 extent 链表）。此时若子进程尝试 `free` 这些继承而来的指针，会操作与父进程共享的元数据结构，导致堆损坏。

`madvise(MADV_WIPEONFORK)` 允许 allocator 在分配关键数据结构时标记对应内存，指示内核在 `fork` 后于子进程中将该区域清零。这样子进程拿到的是干净的匿名页，不再看到父进程的 allocator 元数据。

MossOS 用全局 `WIPEONFORK_AREAS` 表以 `(pid, addr_range)` 为键记录标记区域。`fork` 时 `inherit_wipeonfork_ranges` 将父进程标记复制给子进程，子进程遍历区域逐页分配清零页——对文件映射替换原缓存页，对匿名映射替换 COW 共享的父进程页。`MADV_KEEPONFORK` 从表中移除标记，后续 fork 不再清空该区域。

== /proc/[pid]/maps

调试器（gdb、lldb）、内存分析工具（ASAN、valgrind）和 `pmap` 命令都需要读取 `/proc/[pid]/maps` 来获取进程的内存布局——哪些地址范围被映射、权限是什么、是私有还是共享映射、对应的文件路径。没有这个接口，这些工具无法定位代码段、堆和栈的位置，调试功能基本不可用。

MossOS 的 `mmap_proc_maps()` 取进程地址空间锁后遍历 `MemoryArea`，对每个区域生成一行文本：起始-结束地址、`r/w/x/p` 或 `r/w/x/s` 权限字符串（`p` 表示私有映射，`s` 表示共享映射）、偏移和占位符字段。权限判断依赖 MossOS 在 `MMAP_RECORDS` 表中为每次 `mmap` 存储的 `shared` 属性和 `prot` 标志。`mprotect` 修改权限时同步更新该表中的记录，确保 `maps` 输出与实际权限一致。

== PROT_WRITE 与 SIGBUS

RISC-V 特权架构规范（§4.3.2）要求：当页表项的 W 位为 1 时，R 位也必须为 1。换言之，硬件不允许"只写"页面。用户以 `PROT_WRITE`（不含 `PROT_READ`）调用 `mmap` 时，直接设置 W=1、R=0 会导致 `pt.map()` 在 TLB 刷新阶段触发硬件异常，整个 `mmap` 调用失败。受影响的典型场景包括某些 JIT 编译器分配的 write-only 代码缓冲区。

MossOS 的修复是在 `MmapProt` 到 `MappingFlags` 的转换中做一次补偿：若 `WRITE` 已设置而 `READ` 未设置，自动补上 `READ` 位用于硬件页表，但用户可见的 `MmapProt` 保持不变——调用者后续通过 `mprotect` 或 `/proc/maps` 查询权限时，仍得到 `PROT_WRITE`，而非 `PROT_READ | PROT_WRITE`。

文件映射还有一个类似的语义差异：访问超出文件尾的映射地址时，地址在 `MemoryArea` 范围内是合法的，但后端文件无法提供对应数据。这种情况应触发 `SIGBUS`（总线错误，表示"地址合法但存储不可用"），而非 `SIGSEGV`（段错误，表示"地址根本不在任何映射区域内"）。MossOS 用全局 `SIGBUS_AREAS` 表记录文件映射中超出文件有效范围的区域，缺页处理在发现文件页偏移超出文件长度时先查表确认，若是则触发 `SIGBUS`。进程退出时 `release_process_mmap_resources` 统一清理该进程的 GROWSDOWN、SIGBUS 和 WIPEONFORK 记录。

== COW 与缺页路径的改进

StarryOS 的 COW 机制已通过物理页引用计数实现。MossOS 在此基础上做了两项改进。

引用计数类型从 `u8` 扩展为 `usize`。`u8` 的 255 上限在单进程内足够——同一进程内很少有超过 255 个区域共享同一物理页。但如果进程链式 fork（shell 脚本每行命令 fork 一次），多层父子进程可能累积共享同一物理页，255 的理论上限有被触及的风险。`usize` 消除了这个边界。

Cow Backend 增加 `mapped_pages: AtomicUsize` 计数器，统计当前已装入页表的页面数量。懒加载区域如果从未触发缺页，父页表中就没有需要 COW 保护的物理页。`fork` 时通过原子读取检查该字段，若为零则跳过整个区域的页表遍历和只读保护设置，直接复制区域描述即完成子地址空间的构建。频繁 `fork/exec` 场景（如 shell 脚本循环）中，多数区域在 `fork` 发生时尚未被访问过，跳过无效页表遍历的收益显著。

在缺页路径上，文件映射 Backend 增加了轻量预读探测。用 `last_fault_page` 记录上一次缺页的页号：单页读缺页时若当前页号恰好比上次大 1，判定为顺序扫描，将后续 4 页预热入页缓存。预热不建立页表映射，后续页仍触发缺页来建立映射，但数据已在缓存中，缺页等待时间更短。随机访问时 `last_fault_page` 不连续，预读不触发，单次 fault 延迟不受影响。

Shared Backend 的页面分配也改为惰性策略。StarryOS 的 `SharedPages` 在创建时立即为全部槽位分配物理页——对于上百页的共享映射，即使只实际访问其中几页，也需要承担全部页面的分配开销和 fork 后的引用管理。MossOS 改为 `Option<PhysAddr>` 槽位，物理页在首次 `get_or_alloc` 时才分配，fork 后子进程共享同一组 `SharedPages` 对象，未访问的槽位不占用物理内存。

== 本章小结

MossOS 的内存管理增量集中在 `mmap` 系统调用的语义补全。栈自动增长解决了用户栈动态扩展的需求，避免了大型局部变量和递归场景下的误杀；mremap 重叠处理和 WIPEONFORK 分别消除了数据搬迁中的数据丢失风险和 fork 后 allocator 元数据共享导致的堆损坏；`/proc/[pid]/maps` 为调试器和内存分析工具提供了基本的进程内存可见性；PROT_WRITE 修复和 SIGBUS 追踪补全了 RISC-V 特有的硬件限制和文件映射的边界语义。COW 引用计数扩展和 mapped_pages 跳过优化减少 fork 密集场景的额外开销，缺页预读和 Shared 懒分配分别为顺序访问和共享映射带来了延迟与内存的改善。
