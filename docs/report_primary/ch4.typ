#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(4)

= 内存管理

内存管理为每个进程建立独立的虚拟地址空间，并在程序访问内存时完成地址检查、物理页准备和页表映射。moss 沿用 StarryOS 的主体设计：`AddrSpace` 表示进程地址空间，`MemoryArea` 描述其中的一段合法区域，`PageTable` 保存已经生效的虚实映射，`Backend` 决定页面来自匿名分配、文件、共享内存还是指定物理地址。moss 的修改主要落在 Linux 映射语义、缺页结果和写时复制路径上。

本章按照地址空间的生命周期展开。进程先创建空地址空间，再装入 ELF、堆和栈；运行期间通过 `brk` 和 `mmap` 改变区域布局；页面在首次访问或主动填充时进入页表；`fork` 复制地址空间时使用写时复制；映射删除或进程退出后，相关物理页随引用关系释放。

== 整体架构

=== StarryOS 的已有基础

StarryOS 已经建立了完整的用户虚拟内存骨架。底层 `axalloc` 管理物理页，`axhal` 提供多体系结构页表和地址转换接口，`memory_set` 管理互不重叠的虚拟内存区域。内核在这些组件之上组织进程地址空间，支持 ELF 映射、用户堆栈、匿名与文件 `mmap`、缺页处理和 `fork` 写时复制。

这套设计将“地址是否合法”和“页面是否已经存在”分开处理。一个虚拟地址只要落在某个 `MemoryArea` 中，就属于进程允许使用的区域；如果对应页表项尚未建立，CPU 会触发缺页异常，再由该区域绑定的 Backend 准备页面。按需分页因此不需要在创建区域时一次性分配全部物理内存。

=== moss 的完善范围

moss 没有重建地址空间和页表框架，而是在原有路径中补充 Linux 兼容行为。主要改动如表所示。

#figure(
  table(
    columns: (1.45fr, 2.35fr, 2.75fr),
    align: left,
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([内容], [StarryOS 基线], [moss 的增量]),
    [程序装载], [ELF 段映射、用户栈和堆], [补充动态链接器与脚本路径兼容，缓存 ELF 头部并记录文件有效范围],
    [动态映射],
    [匿名和文件 `mmap`、`munmap`、`mprotect`],
    [补严参数校验，增加 grow-down、fixed-noreplace、映射记录和 `mremap` 行为],

    [缺页结果], [按区域权限调用 Backend 填页], [区分文件尾错误、向下增长区域和非法地址，细化 `SIGBUS` 与 `SIGSEGV`],
    [地址空间复制], [按 Backend 复制或共享映射], [扩大 COW 引用计数，跳过未实际分配的懒加载页面],
  ),
  caption: [内存管理模块的主要增量],
) <tab:mm-overview>

内存管理的核心关系可以概括为：`MemoryArea` 记录一段地址能够怎样使用，Backend 说明缺页时怎样取得页面，页表则保存当前已经建立的映射。三者由 `AddrSpace` 统一管理。

== 地址空间的数据结构

=== AddrSpace

每个进程通过 `ProcessData` 持有一个 `Arc<Mutex<AddrSpace>>`。同一进程内的线程共享该对象，因此看到相同的代码、数据、堆和共享映射；普通 `fork` 创建新的 `AddrSpace`，带有 `CLONE_VM` 的 clone 则继续共享原地址空间。

`AddrSpace` 的主体结构如下：

#figure(
  [
    ```rust
    pub struct AddrSpace {
        // 允许用户进程使用的完整虚拟地址范围
        va_range: VirtAddrRange,
        // 按起始地址组织的虚拟内存区域
        areas: MemorySet<Backend>,
        // 当前已经生效的虚拟页到物理页映射
        pt: PageTable,
    }
    ```
  ],
  caption: [用户地址空间的核心数据结构],
) <lst:addr-space>

`va_range` 先限制整个用户空间的边界，`areas` 再描述其中已经登记的区域，`pt` 只保存已经装入的页面。地址空间操作均围绕这三个字段展开：`map` 增加区域，`unmap` 删除区域和页表项，`protect` 修改权限，`populate_area` 为指定范围准备页面，`handle_page_fault` 处理访问过程中出现的缺页。

=== MemorySet 与 MemoryArea

`MemorySet` 是一组互不重叠的 `MemoryArea`。每个区域保存虚拟地址范围、访问权限和 Backend。例如，代码段通常具有用户态、可读和可执行权限；堆与栈具有用户态、可读和可写权限。

#figure(
  [
    ```rust
    pub struct MemorySet<B: MappingBackend> {
        // 以区域起始地址为键，便于查找相邻区域和检测重叠
        areas: BTreeMap<B::Addr, MemoryArea<B>>,
    }

    pub struct MemoryArea<B: MappingBackend> {
        // 一段连续的虚拟地址范围
        va_range: AddrRange<B::Addr>,
        // 整段区域共享的读、写、执行等权限
        flags: B::Flags,
        // 决定区域中的页面从何处取得
        backend: B,
    }
    ```
  ],
  caption: [MemorySet 与 MemoryArea 的核心字段],
) <lst:memory-area>

#figure(
  table(
    columns: (1.45fr, 2fr, 2.55fr),
    align: left,
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([区域属性], [回答的问题], [示例]),
    [地址范围], [本次访问是否落在已登记区域内], [`0x4000_0000` 到 `0x4001_0000`],
    [映射权限], [本次读、写或执行是否允许], [`READ | WRITE | USER`],
    [Backend], [页表缺页时从哪里取得页面], [匿名分配、文件、共享页或线性映射],
  ),
  caption: [MemoryArea 保存的信息],
) <tab:memory-area>

区域存在不等于页面已经分配。程序可以拥有一个数百页的匿名区域，而页表中只有实际访问过的几页。这样既保留了连续的虚拟地址范围，也避免为未使用页面占用物理内存。

=== PageTable

页表负责把虚拟页转换为物理页，并保存读、写、执行和用户态等权限。以 4 KiB 页面为例，虚拟地址由虚拟页号和页内偏移组成。CPU 先通过页表取得物理页起始地址，再加上原来的页内偏移，得到最终物理地址。

页表不保存文件路径、文件偏移或共享关系。此类信息留在 Backend 中。页表只反映“当前这一页是否可以访问，以及它现在映射到哪里”。因此，同一个 MemoryArea 在运行过程中可以经历未映射、只读共享和私有可写等不同页表状态，而区域本身仍保持不变。

各体系结构使用同一套通用页表结构，但通过类型参数提供页表级数、页表项格式和物理页分配方式。`axhal` 再根据目标架构导出对应的 `PageTable` 类型。

#figure(
  [
    ```rust
    // axhal 根据编译目标选择具体页表类型
    #[cfg(target_arch = "riscv64")]
    pub type PageTable = Sv39PageTable<PagingHandlerImpl>;

    #[cfg(target_arch = "loongarch64")]
    pub type PageTable = LA64PageTable<PagingHandlerImpl>;

    // 多体系结构页表共享的核心结构
    pub struct PageTable64<M, PTE, H> {
        root_paddr: PhysAddr,
        _phantom: PhantomData<(M, PTE, H)>,
    }
    ```
  ],
  caption: [跨体系结构页表的类型组织],
) <lst:page-table>

=== Backend

Backend 将不同来源的页面封装为统一接口。`AddrSpace` 不需要了解文件读取、共享页引用和 COW 复制的细节，只在映射、填页、解除映射和复制地址空间时调用 Backend。

#figure(
  [
    ```rust
    #[derive(Clone)]
    #[enum_dispatch(BackendOps)]
    pub enum Backend {
        // 按固定偏移建立虚拟地址到物理地址的映射
        Linear(LinearBackend),
        // 管理匿名页和私有映射，并在 fork 后执行写时复制
        Cow(CowBackend),
        // 让多个地址空间映射同一组物理页
        Shared(SharedBackend),
        // 将文件缓存页映射到用户地址空间
        File(FileBackend),
    }
    ```
  ],
  caption: [Backend 的四种类型],
) <lst:backend>

#figure(
  table(
    columns: (1.2fr, 2fr, 2.8fr),
    align: left,
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([Backend], [使用场景], [页面来源与处理方式]),
    [`Cow`],
    [堆、栈、私有匿名映射、私有文件映射],
    [首次访问时分配页面；文件映射还会读取对应内容；fork 后按引用数处理写缺页],

    [`File`], [共享文件映射], [使用文件系统提供的缓存页，并在写访问时维护文件页状态],
    [`Shared`], [匿名共享映射和 System V 共享内存], [多个地址空间映射同一组物理页],
    [`Linear`], [信号跳板和设备物理内存], [按照固定虚实地址偏移直接建立映射],
  ),
  caption: [四类映射后端],
) <tab:memory-backends>

Backend 接口中的 `populate` 负责让指定范围满足本次访问权限，`clone_map` 负责地址空间复制。文件页缓存的容量、淘汰和写回属于文件系统的数据管理，本章只讨论 File Backend 怎样把缓存页接入用户页表。

== 用户地址空间的建立

程序装载后，主程序、动态链接器、堆、信号跳板和用户栈共同构成进程的初始虚拟地址空间。#ref(<fig:user-aspace-layout>) 给出了 RISC-V64 与 LoongArch64 下代码中明确规定的地址位置；其中 ELF 各段的实际范围由程序头决定，图中只标出加载基址。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-4.3.1.svg", width: 35%)
    ]
  ],
  caption: [moss 用户虚拟地址空间布局],
) <fig:user-aspace-layout>

=== 创建空地址空间

进程装载程序前先调用 `new_user_aspace_empty`，以体系结构配置中的用户空间起点和长度创建空 `AddrSpace`。此时页表只有根节点，`MemorySet` 中也没有用户区域。RISC-V 等内核与用户共享部分页表的体系结构还需要复制内核地址空间映射；AArch64 和 LoongArch64 使用独立的用户页表寄存器，不执行这一步。

信号跳板是最早加入的区域之一。它使用 Linear Backend，把内核中已有的信号返回代码映射到固定用户地址，并设置为用户可读、可执行。该页面不由用户地址空间分配，因此解除映射时也不会释放其物理内存。

=== ELF 段映射

ELF 加载器遍历所有 `PT_LOAD` 程序段，按照段标志设置读、写和执行权限，再创建文件支持的 Cow Backend。下面代码保留了这一过程的主要步骤：

#figure(
  [
    ```rust
    let backend = Backend::new_cow(
        seg_start,
        PageSize::Size4K,
        FileBackend::Cached(cache.clone()),
        ph.offset,
        Some(ph.offset + ph.file_size),
    );

    uspace.map(
        seg_start.align_down_4k(),
        seg_align_size,
        mapping_flags(ph.flags),
        false, // 只登记区域，不立即装入全部页面
        backend,
    )?;
    ```
  ],
  caption: [ELF 段的懒加载映射],
) <lst:elf-lazy-map>

`ph.mem_size` 决定段在内存中的范围，`ph.file_size` 决定其中有多少内容来自文件。Backend 先分配清零页，再把文件有效区间读入页面，因此文件数据结束后的 BSS 部分保持为零。代码段第一次取指、数据段第一次读取时才真正装入相应页面。

moss 在此路径中加入 ELF 头部缓存，减少频繁 `fork/exec` 时重复读取程序头的开销；同时保存文件有效结束位置，避免把文件尾之后的数据当作正常文件内容。动态链接器、BusyBox 和 shebang 的路径兼容保证了比赛镜像中的 musl、glibc 程序和脚本能够进入同一装载流程。

=== 用户栈与堆

用户栈使用匿名 Cow Backend。加载器先登记整段可读写栈区，再根据 `argv`、`envp` 和辅助向量生成栈内容，只主动填充栈顶实际写入的页面。其余栈页仍保持未映射状态，直到程序访问。

堆从固定的 `USER_HEAP_BASE` 开始。当前实现会在程序装载时建立并填充初始堆区，之后由 `brk` 调整逻辑堆顶。堆扩张超过初始区域时，内核追加匿名 Cow 区域；堆收缩时，超过新堆顶且不属于初始区域的页面被解除映射。

=== 动态内存映射

`mmap` 先检查长度、页对齐、文件描述符、文件权限和映射类型，再寻找空闲虚拟地址并选择 Backend。固定映射使用调用者给出的地址，普通映射则通过 `find_free_area` 在地址空间中寻找连续空洞。

#figure(
  table(
    columns: (2fr, 1.4fr, 2.6fr),
    align: left,
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([映射请求], [Backend], [行为]),
    [`MAP_PRIVATE | MAP_ANONYMOUS`], [`Cow`], [建立私有匿名区域，页面首次访问时清零分配],
    [`MAP_PRIVATE` 文件映射], [`Cow`], [从文件读取初始内容，写入后只修改私有副本],
    [`MAP_SHARED | MAP_ANONYMOUS`], [`Shared`], [预先建立共享物理页集合],
    [`MAP_SHARED` 文件映射], [`File`], [多个映射通过文件缓存观察同一份内容],
    [设备物理范围], [`Linear`], [直接映射设备提供的物理地址],
  ),
  caption: [mmap 标志与 Backend 的对应关系],
) <tab:mmap-backend>

moss 补充了 `MAP_SHARED_VALIDATE`、`MAP_FIXED_NOREPLACE`、`MAP_GROWSDOWN` 和 `MAP_LOCKED` 等标志的处理，并加强文件读写权限与 offset 校验。`mremap` 支持缩小、原地扩张以及 `MREMAP_MAYMOVE` 下的迁移；映射记录还用于生成 `/proc/[pid]/maps` 中的 shared/private 标记。

== 物理页面的建立

=== 按需分页

`AddrSpace::map` 的主要工作是创建 MemoryArea。只有 `populate` 参数为真时，它才立即调用 `populate_area`。通常的 ELF 段、用户栈和普通 `mmap` 都采用懒加载；`MAP_POPULATE`、栈参数和当前初始堆会主动填页。

`populate_area` 按 MemoryArea 边界遍历目标范围，对每一段调用对应 Backend。Backend 最终通过全局页分配器取得物理页，并使用页表接口建立映射。匿名页和 COW 副本被统计为 `VirtMem` 用途，释放时再归还全局页分配器。

=== 不同来源的填页方式

Cow Backend 同时承担匿名分配和私有文件映射。匿名页面从全局分配器取得后清零；文件页面也先清零，再读取该虚拟页对应的文件区间。Shared Backend 在共享对象创建时保存一组物理页，映射时只把它们接入新的页表。Linear Backend 根据固定偏移计算物理地址，不进行内存分配。

File Backend 从文件系统的 Page Cache 取得物理页。共享文件映射首次访问时，Backend 将缓存页映射进用户页表；缓存页以后被文件系统淘汰时，监听器会删除对应页表项。进程再次访问该地址便重新缺页。Page Cache 的组织方式和 I/O 优化在文件系统章节说明。

=== 内核访问用户内存

系统调用读取用户指针时，也可能遇到尚未填充的页面。内核先检查目标范围是否属于可访问的 MemoryArea，再调用 `populate_area` 准备页面。对于跨页的字符串和缓冲区，访问代码允许在受控范围内触发缺页，并由注册的内核态缺页处理器转交当前进程的 `AddrSpace`。

这层检查把用户指针错误和内核自身错误分开：只有当前线程明确处于用户内存访问阶段，内核态缺页才会尝试由用户地址空间处理；其他内核缺页仍按内核异常对待。

== 用户访问与缺页处理

=== 正常地址转换

用户指令给出虚拟地址后，CPU 先查询 TLB，未命中时再遍历页表。有效页表项提供物理页号和访问权限，原虚拟地址的页内偏移保持不变。若映射存在且权限满足，读写或取指直接在物理内存上完成，内核不参与这次访问。

页表项不存在，或者写入只读 COW 页面时，CPU 返回内核并报告故障地址及读、写、执行标志。此时缺页并不等于非法访问，它也可能只是页面尚未装入。

=== 缺页处理流程

#ref(<fig:page-fault-flow>) 展示了用户访问虚拟地址的完整处理路径。映射存在且权限满足时，CPU 直接完成访问；否则进入缺页处理，由 MemoryArea 检查地址范围和权限，再调用该区域绑定的 Backend 准备页面。非法地址或权限错误转为 `SIGSEGV`，合法文件映射无法提供页面时转为 `SIGBUS`。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-ch4缺页.svg", width: 60%)
    ]
  ],
  caption: [用户访问虚拟地址与缺页处理流程],
) <fig:page-fault-flow>

上述流程中的区域检查和 Backend 调用集中在 `AddrSpace::handle_page_fault` 中。区域存在且权限允许时，内核将地址按 Backend 页大小向下对齐，并调用 `populate`：

#figure(
  [
    ```rust
    if let Some(area) = self.areas.find(vaddr) {
        let flags = area.flags();
        if flags.contains(access_flags) {
            let page_size = area.backend().page_size();
            return area.backend()
                .populate(
                    VirtAddrRange::from_start_size(
                        vaddr.align_down(page_size),
                        page_size as usize,
                    ),
                    flags,
                    access_flags,
                    &mut self.pt.modify(),
                )
                .is_ok();
        }
    }
    false
    ```
  ],
  caption: [缺页处理的主体判断],
) <lst:page-fault>

填页成功后，用户上下文返回原指令，CPU 重新执行本次访问。由于页表已经更新，第二次执行通常可以直接完成。若另一个线程已经提前建立了相同映射，Backend 会检查现有页表权限并把该页视为已填充。

=== 缺页失败与信号

用户任务循环根据缺页原因选择信号。地址没有对应 MemoryArea，或区域权限不允许本次访问时发送 `SIGSEGV`；地址属于合法映射，但文件页面无法提供，或者写入被标记为文件总线错误的范围时发送 `SIGBUS`。

#figure(
  table(
    columns: (2.25fr, 1.25fr, 2.5fr),
    align: left,
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([情况], [处理结果], [含义]),
    [页面尚未分配，区域和权限均有效], [重新填页], [正常的按需分页],
    [地址落在 grow-down 预留范围], [扩展映射], [为向下增长区域增加一页],
    [地址没有对应区域或权限不符], [`SIGSEGV`], [进程访问了无效虚拟地址],
    [合法文件映射无法提供页面], [`SIGBUS`], [地址有效，但后端存储对象不能完成访问],
  ),
  caption: [用户缺页的处理结果],
) <tab:page-fault-result>

moss 增加了 `MAP_GROWSDOWN` 的缺页扩展记录，并为部分文件映射和 `mremap` 场景维护 `SIGBUS` 区域。LoongArch64 用户态非对齐异常还会先尝试软件模拟，无法完成时再转为 `SIGBUS`。这些处理使异常结果更接近 Linux 用户程序的预期。

== 地址空间复制与写时复制

=== fork 时的地址空间处理

普通 `fork` 调用 `AddrSpace::try_clone` 创建新地址空间，然后遍历父进程的全部 MemoryArea。每个 Backend 通过 `clone_map` 决定新区域如何取得页面，最后在子进程 `MemorySet` 中登记相同的地址范围和权限。

不同 Backend 的复制策略并不相同。Shared Backend 继续引用同一组物理页；Linear Backend 在子页表中建立相同的线性关系；File Backend 继续使用同一文件缓存来源，但为新的地址空间注册独立的淘汰监听；Cow Backend 则共享已经存在的物理页，并把父子页表都改成只读。

带有 `CLONE_VM` 的 clone 不进入上述过程，而是让新任务直接持有原 `AddrSpace`。这也是同一进程内线程共享内存的实现基础。

=== COW 写缺页

父子进程共享 COW 页面后，任意一方写入都会因为页表缺少写权限而触发缺页。Cow Backend 查询物理页引用计数：

#figure(
  [
    ```rust
    match frame_ref_count {
        1 => {
            // 已无其他地址空间共享，直接恢复写权限
            pt.protect(vaddr, flags)?;
        }
        _ => {
            // 仍被共享，复制数据并改映射到新页面
            let new_frame = alloc_new_frame(false)?;
            copy_page(paddr, new_frame);
            pt.remap(vaddr, new_frame, flags)?;
            drop_old_reference(paddr);
        }
    }
    ```
  ],
  caption: [COW 写缺页的两种处理],
) <lst:cow-fault>

引用数为一说明其他共享者已经退出或复制，当前进程可以直接恢复写权限。引用数大于一时，内核分配新页并复制原数据，只修改当前进程的页表。父子进程从此拥有内容相同但相互独立的页面。

=== moss 的 COW 路径优化

StarryOS 原有 COW 引用计数范围较小，大量 `fork` 可能造成计数溢出。moss 将计数扩展为 `usize`，并在增加引用前检查溢出。

moss 还为 Cow Backend 记录已经装入的页面数量。一个懒加载区域如果从未发生缺页，父页表中就没有需要共享的物理页；`fork` 可以直接复制区域描述，跳过逐页查询。对于已经装入的页面，内核才执行只读保护、引用计数增加和子页表映射。这项修改减少了频繁 `fork/exec` 场景中的无效页表遍历。

== 内存映射的解除与释放

=== 区域删除

`munmap` 按页对齐目标范围，再由 `MemorySet` 处理完整删除、头尾收缩或中间切分。每个被删除的子范围交给 Backend 解除页表映射。`mprotect` 使用相同的区域切分能力，只修改权限而不删除区域；`mremap` 则根据新旧长度收缩、扩张或迁移映射。

`brk` 收缩时只解除初始堆区之外的扩展页面。`execve` 装载新程序前调用 `AddrSpace::clear` 删除旧程序的全部区域，随后重新建立信号跳板、ELF、栈和堆。

=== Backend 的释放责任

#figure(
  table(
    columns: (1.2fr, 2.15fr, 2.85fr),
    align: left,
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([Backend], [解除映射时的操作], [物理页何时释放]),
    [`Cow`], [删除页表项并减少页面引用数], [引用数降为零时归还全局页分配器],
    [`Shared`], [删除当前地址空间的页表项], [最后一个 `Arc<SharedPages>` 释放时统一归还],
    [`File`], [删除用户页表映射], [由文件系统管理缓存页生命周期],
    [`Linear`], [只删除线性页表项], [不释放外部提供的物理内存],
  ),
  caption: [不同 Backend 的页面释放方式],
) <tab:backend-release>

`AddrSpace` 实现了 `Drop`，进程最后一个地址空间引用消失时会调用 `clear`。因此，正常退出、创建失败后的对象回收和 exec 替换最终都汇入同一套区域删除与 Backend 释放流程。

== 本章小结

StarryOS 以 `AddrSpace`、`MemoryArea`、`PageTable` 和 Backend 组成用户内存管理框架。地址空间先登记可用区域，页面再由 Backend 按需装入；CPU 页表负责正常地址转换，缺页处理连接区域描述和物理页面；`fork` 根据 Backend 选择共享、复制或线性重建，私有可写页面通过 COW 延后复制。

moss 保留了这套主体结构，主要补严 `mmap` 与 `mremap` 语义、文件尾和 grow-down 缺页结果，并优化 COW 引用计数和懒加载区域的 fork 路径。文件页在用户页表中的映射由 File Backend 负责，缓存容量、淘汰和写回策略将在文件系统章节继续说明。
