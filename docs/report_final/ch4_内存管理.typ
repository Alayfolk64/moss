#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(4)

= 内存管理

== 概述

StarryOS 已经具备用户虚拟内存管理的主体框架，其核心结构如 @fig:aspace-struct 所示。`AddrSpace` 记录可用地址范围和当前页表；`MemoryArea` 按起始地址组织互不重叠的虚拟区域，每个区域绑定一个 `Backend` 决定页面来源；`Backend` 封装四种映射后端。物理页在缺页时按需装入，`fork` 通过 `AddrSpace::try_clone` 复制区域描述，并对私有可写页面执行 COW 保护。

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
    └── pt: PageTable        已建立的虚拟页 → 物理页映射
    ```
  ],
  caption: [AddrSpace 三层结构概览],
) <fig:aspace-struct>

MossOS 沿用这套框架，主要补充了 `mmap` 的参数校验、映射元数据和部分配套语义。当前 `sys_mmap` 能处理 `MAP_PRIVATE`、`MAP_SHARED` 和 `MAP_ANONYMOUS` 的基本组合，对应关系如 @tab:mmap-backend-base 所示。`madvise`、`mremap` 和 `mlock` 等接口也增加了必要的检查与处理，但部分标志仍属于兼容性实现，例如 `mlock` 尚未真正固定物理页。

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
    [`MAP_SHARED  MAP_ANONYMOUS`], [Shared], [按需分配物理页，多进程共享同一组页面],
    [`MAP_SHARED` 文件映射], [File], [通过文件缓存访问，多映射看到同一份内容],
    [设备物理范围], [Linear], [按固定偏移直接映射设备提供的物理地址],
  ),
  caption: [mmap 标志与 Backend 的对应关系（StarryOS 基线已具备）],
) <tab:mmap-backend-base>

@tab:mmap-additions 总结了 MossOS 当前实现的主要机制。

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
    [栈自动增长], [`MAP_GROWSDOWN` 已定义但忽略], [在映射起点下方的预留窗口内，按缺页地址补充 4 KiB 匿名页],
    [mremap], [仅按匿名映射重建], [支持收缩、原地扩展和迁移；`MREMAP_FIXED` 拒绝新旧范围重叠],
    [WIPEONFORK], [不存在], [fork 后在子进程中清零标记的私有匿名区域],
    [/proc/[pid]/maps], [不存在], [输出地址范围、访问权限以及私有或共享属性],
    [PROT_WRITE], [W=1,R=0 无法形成合法的 RISC-V 页表项], [硬件权限补 READ，用户可见权限保持不变],
    [缺页信号], [未区分失败原因], [根据地址是否仍属于可访问 VMA 选择 SIGBUS 或 SIGSEGV],
  ),
  caption: [MossOS 在 mmap 上的语义补全],
) <tab:mmap-additions>

下面逐项展开这些机制。

== 栈自动增长

用户栈可能因大型局部变量、深层递归或 `alloca` 超出初始映射范围。`MAP_GROWSDOWN` 用于标记可向低地址增长的区域，使这类访问不必立即触发 `SIGSEGV`。

@fig:growdown-flow 展示了当前处理路径。建立 GROWSDOWN 映射时，内核在映射起点下方记录一个最多 256 页的增长窗口。普通缺页处理失败后，`try_growdown_fault` 按 pid 查找窗口；若缺页页地址位于 `[low, high)` 且窗口未被阻断，就以 Cow Backend 建立一个清零的 4 KiB 匿名页。当前实现不修改原 `MemoryArea` 的起始地址，也不要求新增页与已有映射连续。

#figure(
  [
    ```
    handle_page_fault(vaddr)
    ├── areas.find(vaddr) 命中 → 正常 Backend::populate
    └── 未命中
        └── try_growdown_fault(vaddr)
            ├── 查找当前 pid 的增长窗口
            ├── 页地址在 [low, high) 且窗口未阻断?
            │   ├── 是 → 建立一个 4 KiB 私有匿名页
            │   └── 否 → SIGSEGV
            └── MAP_FIXED 与窗口冲突 → 标记窗口为 blocked
    ```
  ],
  caption: [GROWSDOWN 缺页处理流程],
) <fig:growdown-flow>

增长范围由固定的 `low` 下界限制，窗口外访问仍触发 `SIGSEGV`。若后续 `MAP_FIXED` 映射与窗口冲突，`block_growdown` 会禁止该窗口继续补页，避免增长区域侵入其他映射。

== mremap 迁移

当前 `mremap` 支持三条路径：缩小时解除尾部映射；后方空间充足时原地扩展；无法原地扩展且设置 `MREMAP_MAYMOVE` 时，建立新的私有匿名映射、复制原有数据，再解除旧映射。未设置 `MREMAP_MAYMOVE` 且空间不足时返回 `ENOMEM`。

`MREMAP_FIXED` 要求同时设置 `MREMAP_MAYMOVE`，并要求目标地址页对齐。为避免复制和解除映射时相互覆盖，只要源范围与目标范围重叠，当前实现就返回 `EINVAL`。迁移和扩展路径尚未完整保留文件映射、共享属性及原 Backend 类型，因此不能视为完整的 Linux `mremap` 语义。

== MADV_WIPEONFORK

`madvise(MADV_WIPEONFORK)` 指示内核在 `fork` 后清零子进程中的指定区域，可用于避免子进程继承运行库状态、同步数据或敏感信息。当前实现仅允许它作用于私有匿名映射；其他映射类型返回 `EINVAL`。

MossOS 用全局 `WIPEONFORK_AREAS` 表按 pid 和地址范围记录标记。`fork` 时，`inherit_wipeonfork_ranges` 在子地址空间中逐段写零；写入会触发 COW，因此父进程内容不受影响。标记随后继承给子进程，`MADV_KEEPONFORK` 则移除指定范围的标记。

== /proc/[pid]/maps

`/proc/[pid]/maps` 为调试和运行时探测提供进程的基本内存布局。`mmap_proc_maps()` 遍历地址空间中的 `MemoryArea`，输出起止地址、`r/w/x` 权限以及 `p/s` 私有或共享属性。权限和映射类型直接来自每个 VMA 的 `VmaMetadata`，`mprotect` 修改权限时也会同步更新该元数据。

当前偏移、设备号和 inode 仍输出固定占位值，也不输出文件路径，因此这里只提供基础映射列表，尚不等同于 Linux 的完整 `/proc/[pid]/maps`。

== PROT_WRITE 与 SIGBUS

RISC-V 将 W=1、R=0 的页表项编码保留，因而不能直接建立硬件层面的“只写”页面。用户以 `PROT_WRITE`（不含 `PROT_READ`）调用 `mmap` 时，内核仍需生成合法的页表权限。

MossOS 在 `MmapProt` 到 `MappingFlags` 的转换中进行补偿：只要设置 `WRITE`，硬件映射就同时设置 `READ | WRITE`；VMA 元数据仍保存用户请求的权限，因此 `/proc/[pid]/maps` 仍可显示只写权限。

缺页处理还区分地址错误和后端错误：若填页失败，但地址仍位于权限允许的 VMA 内，说明虚拟地址有效而后端无法提供页面，此时发送 `SIGBUS`；地址不属于可访问 VMA 时发送 `SIGSEGV`。此外，`SIGBUS_AREAS` 目前只为共享文件映射的特定 `mremap` 路径记录写缺页范围，并不是通用的文件尾追踪机制。进程退出时会统一清理这些辅助记录。

== 本章小结

MossOS 在既有地址空间框架上补充了多项 `mmap` 配套语义：GROWSDOWN 在受限窗口内处理栈增长缺页，`mremap` 支持收缩、原地扩展和非重叠迁移，WIPEONFORK 在子进程中清零指定私有匿名区域，VMA 元数据为基础版 `/proc/[pid]/maps` 提供权限和映射类型。PROT_WRITE 转换适配了 RISC-V 页表约束，缺页入口也能依据地址是否仍属于有效 VMA 区分 `SIGBUS` 与 `SIGSEGV`。尚未完整实现的映射属性保留、maps 文件信息和页面锁定语义则作为当前限制保留。
