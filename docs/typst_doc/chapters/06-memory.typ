#import "../template.typ": evidence

= 内存管理

== 地址空间模型

用户地址空间由 `mm::AddrSpace` 表示，内部包含三个核心字段：

#table(
  columns: (2.5fr, 3fr, 4.5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*字段*], [*类型*], [*作用*],
  [`va_range`], [`VirtAddrRange`], [限定该地址空间允许映射的用户虚拟地址范围。],
  [`areas`], [`MemorySet<Backend>`], [记录一组虚拟内存区域，每个区域携带权限和映射后端。],
  [`pt`], [`PageTable`], [实际页表，提供 map/unmap/protect/query/clone 时的页表操作。],
)

架构相关常量来自 `kernel/src/config/*.rs`。riscv64/loongarch64 的用户空间范围从 `0x1000` 开始，栈顶为 `0x4_0000_0000`；x86_64/aarch64 的用户空间范围更大，栈顶为 `0x7fff_0000_0000`。初始堆基址均为 `0x4000_0000`，`brk` 最大扩展范围由 `USER_HEAP_SIZE_MAX` 限制。

== 映射后端

`AddrSpace` 不直接把所有区域映射成匿名页，而是通过 `Backend` 区分来源和行为。

#table(
  columns: (2fr, 3fr, 5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*后端*], [*路径*], [*用途*],
  [`Linear`], [`backend/linear.rs`], [虚拟地址和物理地址固定偏移，设备 mmap 中的物理 framebuffer 映射会使用该后端。],
  [`Cow`], [`backend/cow.rs`], [私有文件映射和 fork/clone 地址空间复制时的写时复制；内部维护 frame refcount。],
  [`Shared`], [`backend/shared.rs`], [匿名 shared mmap 和 SysV shared memory 使用的共享物理页数组。],
  [`File`], [`backend/file.rs`], [共享文件 mmap，连接 `CachedFile`，缺页时从页缓存取页，并支持简单 readahead 与 evict listener。],
)

`BackendOps::populate` 使映射可以先建立 VMA，实际物理页在缺页或显式 populate 时分配/装入。`mmap` 的 `MAP_POPULATE` 会立即调用 populate；普通匿名映射、文件映射和栈映射可以按需填充。

== ELF 装载

`kernel/src/mm/loader.rs` 负责把用户程序装入地址空间。主要流程如下：

```text
resolve_exec_path()
ElfLoader::load()
  -> 读取 ELF headers 并缓存
  -> 如有 PT_INTERP，解析动态链接器并 fallback 到 /glibc 或 /musl
  -> uspace.clear()
  -> map_trampoline()
  -> map_elf() 映射 LOAD 段
  -> 映射动态链接器
  -> 构造 auxv
load_user_app()
  -> 映射用户栈
  -> 写入 argv/envp/auxv 栈数据
  -> 映射初始 heap
```

ELF `LOAD` 段使用 `Backend::new_cow` 做文件后端私有映射，支持懒装入和写时复制。信号 trampoline 被映射到 `SIGNAL_TRAMPOLINE`，权限为用户可读可执行。

== 缺页处理

用户任务运行在 `kernel/src/task/user.rs::new_user_task` 创建的循环中。`UserContext::run()` 返回 `PageFault(addr, flags)` 时，内核先检查 mmap 记录是否应产生 `SIGBUS`，再调用 `AddrSpace::handle_page_fault` 或 `try_growdown_fault`。成功处理则继续用户态；否则按地址和权限情况发送 `SIGBUS` 或 `SIGSEGV`。

`AddrSpace::handle_page_fault` 会确认 fault 地址在用户范围内，并且对应 VMA 权限包含访问类型；随后按后端 page size 对齐 fault 页，调用后端 `populate`。如果后端成功填充至少一页，fault 被视为已处理。

== mmap 与 brk

`sys_mmap` 支持 `MAP_PRIVATE`、`MAP_SHARED`、`MAP_SHARED_VALIDATE`、`MAP_ANONYMOUS`、`MAP_FIXED`、`MAP_FIXED_NOREPLACE`、`MAP_GROWSDOWN`、`MAP_LOCKED`、`MAP_POPULATE` 等标志的解析。它根据映射类型选择 CoW、Shared、File、Linear 或 Alloc 后端，并记录 mmap 区间用于 `/proc/pid/maps`、growdown fault 和部分 SIGBUS 语义。

`sys_brk` 使用 `ProcessData.heap_top` 保存当前堆顶。扩展堆时只在超过初始 heap 区域后映射新页面，收缩时只回收初始 heap 之外的页。返回值保持 Linux 风格：非法或失败时返回旧堆顶。

== 用户指针访问

用户指针访问集中在 `kernel/src/mm/access.rs`。`UserPtr<T>` 表示可写用户指针，`UserConstPtr<T>` 表示只读用户指针。二者通过 `check_region` 检查对齐、地址空间权限并调用 `populate_area`，null-terminated 字符串通过逐页检查避免越界读取。`access_user_memory` 会设置线程上的 `accessing_user_memory` 标记，让内核访问用户内存时的缺页路径可被识别。

== 代码依据

#evidence((
  [`地址空间`], [`kernel/src/mm/aspace/mod.rs`], [`AddrSpace` 包含 `va_range`、`MemorySet<Backend>` 和 `PageTable`，提供 map/unmap/protect/handle_page_fault/try_clone。],
  [`映射后端`], [`kernel/src/mm/aspace/backend/mod.rs`], [`Backend` 枚举包含 `Linear/Cow/Shared/File`，并实现 `MappingBackend`。],
  [`ELF 装载`], [`kernel/src/mm/loader.rs`], [`map_elf` 使用 CoW 文件映射，`load_user_app` 映射栈、heap 并写入 argv/envp/auxv。],
  [`mmap/brk`], [`kernel/src/syscall/mm/mmap.rs`, `kernel/src/syscall/mm/brk.rs`], [`sys_mmap` 选择后端并记录映射，`sys_brk` 维护 `heap_top`。],
  [`缺页路径`], [`kernel/src/task/user.rs`], [`ReturnReason::PageFault` 调用地址空间缺页处理，失败时产生 `SIGBUS` 或 `SIGSEGV`。],
))
