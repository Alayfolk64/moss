#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(4)

= 内存管理

== 整体架构

StarryOS 已经具备用户虚拟内存管理的主体框架，支持程序装载、用户堆栈、动态映射、缺页处理和写时复制。moss 沿用这一设计，没有重新构造内存管理体系，而是重点完善 Linux 映射语义、缺页异常处理和频繁 `fork` 场景下的 COW 路径，并加入全局文件页缓存以减少重复 I/O，进而有效提升了 I/O 性能。

整体上，系统采用虚拟内存和分页机制隔离不同进程，将连续的虚拟地址按页映射到离散的物理内存。程序装载、堆栈和动态映射首先确定可用地址范围与访问权限，物理页通常推迟到首次访问时分配；发生缺页后，内核再根据区域用途从匿名内存、文件缓存、共享内存或指定物理地址取得页面。进程复制通过写时复制延后私有页面的实际拷贝，文件页则借助全局缓存复用已经读入的内容。映射解除或进程退出时，系统依据页面的所有权和引用关系回收资源，由此形成地址空间建立、按需填页、共享复制和释放回收的完整过程。

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

`MemorySet` 是一组互不重叠的 `MemoryArea`。每个`MemoryArea`保存虚拟地址范围、访问权限和`Backend`。例如，代码段通常具有用户态、可读和可执行权限；堆与栈具有用户态、可读和可写权限。

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

区域存在不等于页面已经分配。程序可以拥有一个数百页的匿名区域，而页表中只有实际访问过的几页。这样既保留了连续的虚拟地址范围，也避免为未使用页面占用物理内存。

=== PageTable

页表负责把虚拟页转换为物理页，并保存读、写、执行和用户态等权限。以 4 KiB 页面为例，虚拟地址由虚拟页号和页内偏移组成。CPU 先通过页表取得物理页起始地址，再加上原来的页内偏移，得到最终物理地址。

页表不保存文件路径、文件偏移或共享关系。此类信息留在 `Backend` 中。页表只反映“当前这一页是否可以访问，以及它现在映射到哪里”。因此，同一个 `MemoryArea` 在运行过程中可以经历未映射、只读共享和私有可写等不同页表状态，而`MemoryArea`本身仍保持不变。

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

装载程序前，内核先调用 `new_user_aspace_empty` 创建一个空的用户地址空间。此时还没有代码段、堆和栈，只有管理虚拟地址所需的基础页表。加载器随后向其中加入信号跳板、ELF 程序段、用户栈和用户堆，进程的地址空间才逐步形成。

信号跳板是一小段由内核提供的代码。进程收到信号后会暂时转去执行信号处理函数，处理结束时再通过这段代码调用 `rt_sigreturn`，让内核恢复收到信号前的寄存器和执行位置。moss 将信号跳板映射到固定的用户地址，并设置为只读、可执行。它使用内核中已有的物理页，因此删除地址空间时只需移除映射，不需要释放这页内存。

=== ELF 段映射

ELF 文件中保存着程序代码、数据以及装载说明。装载说明以“程序段”的形式记录，其中类型为 `PT_LOAD` 的程序段表示必须放入用户地址空间的内容，例如代码段或数据段。加载器逐个处理这些可装载段，并按照 ELF 中记录的权限建立映射。

#figure(
  [
    ```rust
    // ph 是一个 ELF 程序段描述。
    // PT_LOAD 表示该段必须装入用户地址空间。
    for ph in program_headers
        .iter()
        .filter(|ph| ph.get_type() == Ok(Type::Load))
    {
        // virtual_addr 是段在程序中的虚拟地址，
        // elf_base 是主程序或动态链接器的加载基址。
        let vaddr = ph.virtual_addr as usize + elf_base;
        let seg_start = VirtAddr::from_usize(vaddr);

        // 段起点可能没有按页对齐，seg_pad 保存页内偏移。
        // mem_size 是该段装入内存后的总大小。
        let seg_pad = vaddr.align_offset_4k();
        let seg_align_size =
            align_up_4k(ph.mem_size as usize + seg_pad);

        let backend = Backend::new_cow(
            seg_start,
            PageSize::Size4K,
            // cache 保存 ELF 文件的数据页。
            FileBackend::Cached(cache.clone()),
            // offset 是段内容在 ELF 文件中的起始位置。
            ph.offset,
            // file_size 是文件中实际存在的数据大小；
            // mem_size 多出的部分不读取文件，保持为零。
            Some(ph.offset + ph.file_size),
        );

        uspace.map(
            seg_start.align_down_4k(),
            seg_align_size,
            // 根据段标志设置读、写、执行权限。
            mapping_flags(ph.flags),
            false, // 只登记区域，不立即装入物理页
            backend,
        )?;
    }
    ```
  ],
  caption: [ELF 段的懒加载映射],
) <lst:elf-lazy-map>

建立映射时，内核只记录这段地址可以怎样访问，并不立即把整个程序读入物理内存。代码第一次执行或数据第一次读取时，Backend 才分配清零页，并从 ELF 文件中填入对应内容。文件中没有保存实际数据的 BSS 部分因此自然保持为零。

moss 在此路径中加入 ELF 头部缓存，减少频繁 `fork/exec` 时重复读取程序头的开销；同时保存文件有效结束位置，避免把文件尾之后的数据当作正常文件内容。动态链接器、BusyBox 和 shebang 的路径兼容保证了比赛镜像中的 musl、glibc 程序和脚本能够进入同一装载流程。

=== 用户栈与堆

用户栈保存函数调用产生的局部状态。程序启动时，加载器还会将命令行参数、环境变量和辅助向量写入栈顶。内核先登记整段可读写栈区，只为启动数据实际占用的页面分配物理内存，其余页面留待首次访问时填充。

用户堆承载运行期间的动态内存分配。moss 将堆起点固定为 `USER_HEAP_BASE`，装载程序时建立 64 KiB 的初始堆，之后由 `brk` 调整堆顶。堆扩张时，内核为新增范围建立可读写的匿名映射；堆收缩时，解除新堆顶以外的扩展映射。栈和堆均使用 Cow Backend，页面首次访问时清零分配，`fork` 后通过写时复制暂时共享。

=== 动态内存映射

`mmap` 用于在现有代码段、堆和栈之外建立新的虚拟内存区域。moss 中对应的系统调用定义如下：

#figure(
  [
    ```rust
    pub fn sys_mmap(
        addr: usize,   // 建议使用的虚拟地址；固定映射时为目标地址
        length: usize, // 映射区域的长度
        prot: u32,     // 读、写和执行权限
        flags: u32,    // 私有、共享、匿名或固定映射等方式
        fd: i32,       // 文件映射使用的文件描述符
        offset: isize, // 映射内容在文件中的起始偏移
    ) -> AxResult<isize>
    ```
  ],
  caption: [mmap 系统调用的参数定义],
) <lst:mmap-definition>

内核收到请求后，先检查映射长度、页对齐、文件描述符、文件权限和映射类型，再确定虚拟地址并选择 Backend。固定映射使用调用者给出的地址，普通映射则通过 `find_free_area` 在地址空间中寻找连续空洞。

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

moss 补充了 `MAP_SHARED_VALIDATE`、`MAP_FIXED_NOREPLACE`、`MAP_GROWSDOWN` 和 `MAP_LOCKED` 等标志的处理，并加强文件读写权限与 offset 校验。`mremap` 用于调整已有映射的大小：缩小时，内核解除末尾多余的映射；扩张时，优先使用原区域后方的空闲地址。若后方空间不足且调用者设置了 `MREMAP_MAYMOVE`，内核可以将映射迁移到新的空闲区域。moss 还记录每次 `mmap` 创建的地址范围及其共享或私有属性，供 `/proc/[pid]/maps` 输出相应的 `s`（共享映射） 或 `p`（私有映射） 标记。

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

== 全局文件页缓存

全局文件页缓存是 moss 在 StarryOS 基础上的一项重要改动。原有的文件缓存以单个文件为中心，难以统一控制不同文件占用的物理页数量；moss 增加 `FILE_CACHES`（文件缓存对象表） 和 `GLOBAL_PAGE_CACHE`（文件数据页池），使普通文件的读写、程序装载和文件映射能够复用同一批缓存页。缓存以“文件系统对象、inode、文件页号”定位页面，其中 `FILE_CACHES` 保证同一文件复用同一个 `CachedFileRecord`，`GLOBAL_PAGE_CACHE` 统一保存普通文件的数据页，并按 LRU 策略决定容量不足时淘汰哪一页。

=== 全局组织方式

#figure(
  [
    ```rust
    const PAGE_SIZE: usize = 4096;
    // 全局最多缓存 16K 个普通文件页，共 64 MiB
    const GLOBAL_FILE_CACHE_PAGES: usize = 16 * 1024;

    // 同一文件只建立一份缓存记录
    static FILE_CACHES:
        RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>>;

    // 所有普通文件的数据页共用一个全局缓存；
    // 内部的 LruCache 维护最近使用顺序，便于使用 LRU 算法进行页面淘汰。
    static GLOBAL_PAGE_CACHE: Mutex<GlobalPageCache>;

    struct GlobalPageKey {
        cache_id: u64, // 文件缓存记录
        pn: u32,       // 文件内页号
    }
    ```
  ],
  caption: [全局文件页缓存的核心组织],
) <lst:global-page-cache>

访问文件页时，`CachedFile` 先以文件和页号查询全局文件页缓存。命中时直接使用已有缓存页，并更新其最近使用位置；未命中时才分配一页物理内存并从文件读取数据，随后将其加入全局缓存。不同进程打开同一文件、重复执行同一程序或者映射同一共享库时，均可以命中此前留下的文件页，从而减少块设备访问。缓存还提供最大 256 页的显式预读接口；文件映射出现连续读缺页时，File Backend 会自动预取后续 4 页。

=== 程序装载与共享库复用

ELF 装载器通过 `CachedFile::get_or_create` 获取主程序和动态链接器的缓存对象，动态链接器加载 libc 等共享库时，文件 `mmap` 也进入同一缓存体系。因此，共享库某一文件页首次使用时可能需要从存储设备读入；只要该页仍在全局缓存中，其他进程再次使用相同内容时就不必重复读取磁盘。

全局缓存减少的是共享库文件页的重复 I/O。每个动态链接程序仍会完成共享库映射、符号解析和重定位；当 libc 等共享库的文件页已经留在缓存中时，后续进程处理相同页面时可以直接从内存取得内容。对于私有文件映射，Cow Backend 仍会为进程建立私有物理页；对于共享文件映射，File Backend 可以将同一个缓存物理页接入多个进程页表。因此，缓存的主要作用是把后续文件缺页中的设备读取转化为内存访问。

=== 淘汰、写回与映射同步

全局缓存达到 64 MiB 上限后，内核按照 LRU 策略选择较久未访问的页面。干净页可以直接释放，脏页先写回文件；连续脏页最多按 16 页、64 KiB 合并写回，减少零散 I/O。`fsync`、全局同步和主动清理缓存也会触发写回。

共享文件页可能已经出现在多个进程的页表中。File Backend 因此为缓存注册淘汰监听器：页面被移出缓存前，缓存先通知相关地址空间删除页表项，再写回脏数据并释放物理页。进程以后再次访问该虚拟地址时重新缺页，由 Backend 获取新的缓存页。该机制使文件缓存和用户页表保持一致，避免页表继续引用已经回收的物理内存。

全局缓存的收益主要体现在三个方面：第一，同一文件的数据页可以跨进程复用，这样能够减少程序和共享库的重复读取；第二，统一的缓存容量和 LRU 淘汰顺序可以在不同文件之间重新分配页面，避免每个文件长期保留固定数量的缓存页；第三，读写、程序装载与共享文件映射使用同一份缓存内容，配合预读和批量写回可以降低 I/O 次数，有效提升了性能。

== 用户访问与缺页处理

=== 正常地址转换

用户指令给出虚拟地址后，CPU 先查询 TLB，未命中时再遍历页表。有效页表项提供物理页号和访问权限，原虚拟地址的页内偏移保持不变。若映射存在且权限满足，读写或取指直接在物理内存上完成，内核不参与这次访问。

若页表项不存在，或者写入只读 COW 页面时，CPU 返回内核并报告故障地址及读、写、执行标志。此时缺页并不等于非法访问，它也可能只是页面尚未装入。

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
    // 先根据故障地址找到所属的 MemoryArea。
    // 找不到区域说明该地址不属于进程的合法虚拟空间。
    if let Some(area) = self.areas.find(vaddr) {
        let flags = area.flags();

        // 本次访问的类型必须被区域权限允许，
        // 例如写访问需要区域本身带有 WRITE 权限。
        if flags.contains(access_flags) {
            let page_size = area.backend().page_size();

            // Backend 以页为单位填充，因此先把故障地址向下对齐。
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

    // 地址不存在或权限不匹配时，缺页处理失败。
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

普通 `fork` 调用 `AddrSpace::try_clone` 创建新的地址空间。内核先复制父进程的虚拟区域描述，使子进程拥有相同的代码、数据、堆、栈和映射布局；随后再处理已经装入页表的页面。对于私有可写页面，父子进程暂时共享同一物理页，并把页表权限降为只读。这样 `fork` 不需要立即复制整个进程内存，真正的数据复制推迟到后续写入时完成。

文件映射和共享映射也在这一阶段保留原有的共享关系。文件映射继续使用同一文件缓存来源，并为子地址空间注册独立的缓存淘汰监听；共享内存继续指向同一组物理页。子进程因此得到与父进程一致的虚拟布局，但是否共享物理页由映射类型和后续访问决定。

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

moss 还为 Cow Backend 记录已经装入的页面数量。一个懒加载区域如果从未发生缺页，父页表中就没有需要共享的物理页，`fork` 可以直接复制区域描述，跳过逐页查询。对于已经装入的页面，内核才执行只读保护、引用计数增加和子页表映射。这项修改减少了频繁 `fork/exec` 场景中的无效页表遍历。

== 内存映射的解除与释放

=== 区域删除

区域删除的主路径由 `munmap` 触发。moss 在系统调用入口取得当前进程的 `AddrSpace` 锁，将长度向上按页对齐，然后调用 `AddrSpace::unmap`。这一层只做地址范围校验，确认待删除范围仍处在用户地址空间中；通过校验后，删除工作交给 `MemorySet::unmap`。

`MemorySet` 内部用有序结构保存互不重叠的 `MemoryArea`，因此删除一段范围时不需要逐页扫描整个地址空间。它先删除完全落在目标范围内的区域，再处理左右边界上的相交区域。边界处理分三类：删除范围覆盖区域尾部时收缩右端，覆盖区域头部时收缩左端；删除范围位于区域中间时，原区域会被切成左右两个区域。每次区域收缩或删除时，底层页表映射都会同步解除。

#figure(
  ```rust
  pub fn unmap(
      &mut self,
      start: B::Addr,
      size: usize,
      page_table: &mut B::PageTable,
  ) -> MappingResult {
      let range = AddrRange::try_from_start_size(start, size)
          .ok_or(MappingError::InvalidParam)?;

      // 删除完全位于目标范围内的 MemoryArea。
      self.areas.retain(|_, area| {
          if area.va_range().contained_in(range) {
              area.unmap_area(page_table).unwrap();
              false
          } else {
              true
          }
      });

      // 左边界相交：收缩区域尾部，或先切分再收缩。
      // 右边界相交：收缩区域头部。
      // 收缩与切分过程中都会调用 backend.unmap 清除页表项。
      ...
  }
  ```,
  caption: [MemorySet 的区域删除逻辑],
) <code:memoryset-unmap>

`mprotect` 与 `mremap` 复用了这套区域切分能力。`mprotect` 不删除区域，只在切分后修改目标子区域的权限和页表标志；`mremap` 缩小时调用 `unmap` 删除末尾范围，扩张时优先在原区域后方追加映射，空间不足且允许移动时才迁移到新区域。`brk` 收缩也会进入区域删除路径，但只解除初始堆区之外的扩展页面。`execve` 和地址空间析构则调用 `AddrSpace::clear`，一次性删除全部区域。

`AddrSpace` 实现了 `Drop`，进程最后一个地址空间引用消失时会调用 `clear`。因此，正常退出、创建失败后的对象回收和 exec 替换最终都汇入同一套区域删除与 Backend 释放流程。

== 本章小结

StarryOS 以 `AddrSpace`、`MemoryArea`、`PageTable` 和 Backend 组成用户内存管理框架。地址空间先登记可用区域，页面再由 Backend 按需装入；CPU 页表负责正常地址转换，缺页处理连接区域描述和物理页面；`fork` 根据 Backend 选择共享、复制或线性重建，私有可写页面通过 COW 延后复制。

moss 保留了这套主体结构，主要补严 `mmap` 与 `mremap` 语义、文件尾和 grow-down 缺页结果，并优化 COW 引用计数和懒加载区域的 fork 路径。文件页在用户页表中的映射由 File Backend 负责，缓存容量、淘汰和写回策略将在文件系统章节继续说明。
