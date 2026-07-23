# Moss 当前缺页处理：流程、原理与逐行导读

本文只解释当前工作区中的实际代码，不把设计目标当成已经实现的效果。阅读基准是
2026-07-23 的 `smp-scheduler-optimization` 分支；当前 `HEAD` 是
`8b0637a0bd51384f9e0e3acdc52437349574a5b8`，但相关文件还有未提交修改，因此应以
本文列出的源码路径和当前工作区内容为准。

重点源码：

- `vendor/axcpu/src/{riscv,loongarch64}/uspace.rs`：把硬件异常翻译成缺页原因。
- `kernel/src/task/user.rs`：用户态缺页的总入口和信号选择。
- `kernel/src/mm/access.rs`：内核访问用户地址时的缺页入口。
- `kernel/src/mm/aspace/mod.rs`：地址检查、两阶段缺页流程、回退路径和 TLB/取指同步。
- `kernel/src/mm/aspace/backend/{mod,file,cow}.rs`：缺页计划及文件映射、私有映射的实现。
- `vendor/page_table_multiarch/src/bits64.rs`：并发安装或替换 4 KiB PTE。
- `vendor/axfs/src/highlevel/file.rs`：Moss 当前的每 inode folio 页缓存。

本文先建立全局模型，再逐行读关键代码。为了真的解释到“每一个单词”，后文先统一
解释 Rust 语法词和命名缩写；逐行部分再解释每个本地标识符。这样无需在每一行重复
解释 `let`、`&`、`::` 等相同语法。

## 一、先看完整流程

### 1. 什么是这里的“缺页”

CPU 执行用户程序时，用虚拟地址访问内存。页表项（PTE）可能不存在，也可能存在但
缺少本次访问所需的读、写或执行权限。硬件此时停止当前用户指令，保存现场并进入
内核。Moss 将这次异常表示成：

```text
PageFault(发生访问的虚拟地址, READ/WRITE/EXECUTE | USER)
```

“缺页”不一定表示物理内存不足，也不一定需要磁盘 I/O。当前代码会遇到至少四种情况：

1. 页已经被另一个 CPU 映射，只是本 CPU 仍因旧的 TLB 状态进入了异常。
2. 匿名页或私有文件页尚未分配，需要分配并填零或从文件读取。
3. COW 页存在但只读，本次是写访问，需要复制后换成私有可写页。
4. 共享文件页尚未映射，需要从文件页缓存取得 folio 子页并安装 PTE。

地址不属于有效 VMA、权限不允许，或者文件页装载失败时，这次异常不能被正常修复，
最终会转成 `SIGSEGV` 或 `SIGBUS`。

### 2. 当前主流程

```mermaid
flowchart TD
    A[用户指令触发硬件页异常] --> B[架构代码生成地址和 READ/WRITE/EXECUTE | USER]
    B --> C[user.rs 先检查显式 SIGBUS 区域]
    C -->|不是显式 SIGBUS| D[AddrSpace 读锁：检查地址、VMA、权限并 prepare]
    D --> E{FaultPreparation}
    E -->|Handled| K[刷新本地 TLB；执行缺页时同步取指]
    E -->|File/Cow plan| F[释放 AddrSpace 锁]
    F --> G[分配页、复制或读取 folio]
    G --> H[重新拿 AddrSpace 读锁并重新检查 VMA/权限]
    H --> I[用原子 PTE 操作 commit]
    I -->|Handled| K
    I -->|Stale| J[拿 AddrSpace 写锁走旧 populate 路径]
    E -->|Fallback| J
    J -->|成功| K
    J -->|失败| L[尝试 MAP_GROWSDOWN]
    L -->|仍失败| M[按当前地址可访问性选择 SIGBUS 或 SIGSEGV]
    K --> N[返回用户态，重试原指令]
```

最关键的结构是 `prepare → execute → commit`：

- `prepare` 很短，只在地址空间读锁下观察 VMA、权限和当前 PTE，生成一个计划。
- `execute` 不持有地址空间锁，执行可能很慢的分配、内存复制或文件读取。
- `commit` 重新拿读锁，确认 VMA 和 PTE 没有变，再用原子操作安装结果。
- 如果执行期间另一个线程改了映射，结果叫 `Stale`，不会把旧计划强行写入新映射。

### 3. 为什么要分成三段

旧路径 `AddrSpace::handle_page_fault(&mut self, ...)` 需要 `&mut AddrSpace`，调用者必须
持有整个进程地址空间的独占锁。若它在锁内等待块设备，那么同一进程中其他 CPU 即使
缺的是完全不同的页，也不能进入这段代码。

当前 File/COW 的 4 KiB 快路径把慢操作移出了地址空间锁：

```text
短读锁观察状态 → 无地址空间锁做慢操作 → 短读锁 + PTE CAS 提交
```

这里没有简单地“去掉锁”。地址空间结构仍然需要锁，否则 `munmap`、`mprotect`、
VMA 拆分和缺页提交会互相踩坏。当前做法是缩小保护范围，并用“提交前重新验证”处理
释放锁期间发生的并发变化。

### 4. 当前 folio 是什么

当前 `vendor/axfs/src/highlevel/file.rs` 中的 folio 是 Moss 自己的文件页缓存单位：

- 一个 folio 包含 `2^order` 个连续 4 KiB 基页。
- `MAX_FOLIO_ORDER = 4`，所以当前最大是 16 页，即 64 KiB。
- folio 的起始文件页号按自身大小自然对齐。
- `folio_index` 让 folio 中每个 4 KiB 页号都能查到同一个 `Arc<Folio>`。
- `folio_heads` 为每个 folio 保存一份规范拥有者引用。
- `FolioState.loading` 表示某个任务正在装载；其他任务找到它后等待同一次装载，而不再
  发起一份相同 I/O。
- `FolioPageRef` 固定其中一个 4 KiB 子页，使 PTE 指向它时物理帧不会被页缓存回收掉。

这借用了 Linux `address_space` 和 folio 的核心思想，但不是 Linux folio 子系统的完整
移植。当前没有 Linux XArray、完整的 readahead state、所有 writeback 状态机和内存
回收层级；Moss 使用的是分片 `BTreeMap`、自己的状态位和水位回收。

### 5. 文件映射和 COW 映射分别怎么走

#### 共享文件映射 `FileBackend`

缺少 PTE 时，`prepare_fault` 记录虚拟页和文件页号。`execute` 调
`fault_page_ref(pn)`：

1. 命中缓存就取得现有 folio。
2. 未命中就发布一个 `loading` folio。
3. 发布者读取整个自然对齐 folio；竞争者等待这个 folio 变为 `uptodate`。
4. 返回故障所在的 4 KiB `FolioPageRef`。

`commit_fault` 再验证后把这个子页物理地址写进 PTE，同时把 `FolioPageRef` 放进
`mapped_pages` 固定住它。

对磁盘文件，普通读缺页先映射成只读；写缺页会在有效性保护下标脏并按 VMA 权限映射。
对 tmpfs，`in_memory()` 为真，代码直接使用原始 VMA 权限。

#### 私有映射 `CowBackend`

它覆盖匿名懒分配、`MAP_PRIVATE` 文件映射和 fork 后写时复制：

- 未映射匿名页：无锁阶段分配并清零一个页，提交阶段映射。
- 未映射私有文件读页：优先直接只读映射文件缓存页；第一次写再复制。
- 已映射共享匿名页的写缺页：额外固定源页，无锁复制，提交时检查 PTE 仍指向原页后替换。
- 已映射文件缓存页的写缺页：固定 `FolioPageRef`，无锁复制，提交时原子替换并删除缓存页
  pin。

私有文件读缺页还会在一个按文件偏移对齐的 64 KiB 窗口内，把已就绪的相邻缓存页一并
映射为只读。必须注意：当前 `fault_page_ref` 会同步等待整个被选中的 folio 装载完成。
`CowFaultPlan` 注释中的“remainder ... asynchronously”与实际调用链并不完全一致；
真正的异步预读是独立的 `CachedFile::readahead()` 工作者路径。本文以代码行为为准，
不把这段 demand folio load 描述成异步 I/O。

### 6. 这样做的实际好处

下面只写能由当前代码直接推出的收益：

1. **不同页的慢操作不再被进程级地址空间写锁完全串行化。** File/COW 4 KiB 快路径的
   分配、复制和文件读取发生在 `AddrSpace` 锁外。
2. **相同文件 folio 的并发缺页合并为一次装载。** 先发布 `loading` folio；后来的
   任务等待它，而不是重复读。
3. **私有文件的首次读可以少一次内存复制。** 完整缓存页以只读方式直接映射，写时再
   COW。
4. **一个 64 KiB folio 内可能减少后续异常和重复存储请求。** demand load 得到整
   个 folio，COW fault-around 会把当时已经有效的邻页 PTE 一起装上。
5. **竞争提交不会覆盖胜者。** PTE 通过 compare-and-exchange 安装或条件替换；输掉
   竞争的计划被释放或回退。
6. **页缓存不再固定限制为 256 MiB。** 当前按可用页水位触发回收：低水位约为总页数
   的 1/16、目标约为 1/8，并至少保留 64 MiB 可用内存。
7. **可执行页在修复后有架构同步。** RISC-V 执行 `fence.i`；LoongArch 执行
   `ibar 0`，避免刚生成的 tmpfs 可执行文件立即运行时使用旧取指状态。

这些机制“允许”更多并行，并不自动证明 BuildStorm 一定快多少。性能幅度仍取决于
ext4 锁、块设备队列、调度器、QEMU 翻译速度、Cargo 依赖图和工作负载命中率，需要
运行数据才能量化。

### 7. 当前仍然存在的串行和限制

1. `LinearBackend`、`SharedBackend`、非 4 KiB COW、特殊 PTE 状态和竞争变旧的计划
   仍走 `Fallback`，持有地址空间独占锁调用旧 `populate`。
2. `prepare` 和 `commit` 仍持有同一进程的地址空间**读锁**。读锁可彼此并发，但会
   与 `mmap/munmap/mprotect` 等写锁互斥。
3. 同一个 PTE 由 CAS 串行决定唯一胜者；这是正确性所需，不是应当消除的全局锁。
4. 同一个 folio 的装载只有一个 loader，其他访问者等待。它消除了重复 I/O，但同一
   folio 的首次访问仍受那次 I/O 延迟约束。
5. `folio_heads` 在发布/删除一个 folio 时是每 inode 的互斥锁；缓存命中的
   `lookup_folio` 不拿它。
6. 每个 4 KiB 子页有自己的 `page_locks[page]`；同一子页的读写会串行，不同子页可
   分开。
7. truncate/破坏性 invalidation 通过每 inode 的 `invalidate_lock` 写侧排除读者；
   普通缺页拿读侧。
8. ext4 元数据和底层块 I/O 仍有自己的锁与等待。folio 只改变文件页缓存与缺页层，
   不能据此声称 ext4 已经完全无全局锁。
9. 共享文件并发快路径只提交故障的一个 4 KiB PTE；它的 demand folio 可以包含邻页，
   但不像 COW fault-around 那样在同一次提交中遍历安装整个窗口。
10. 当前 `SIGBUS_AREAS` 的提前判断只检查记录区域内的**写**缺页，不是完整复刻 Linux
    的所有 mmap `SIGBUS` 语义。

## 二、读代码前的词汇和 Rust 语法

### 1. 内存管理词汇

| 词 | 当前代码中的含义 |
| --- | --- |
| VA / `vaddr` | virtual address，虚拟地址。 |
| PA / `paddr` | physical address，物理地址。 |
| VMA / `area` | 一段连续虚拟地址区域及其权限、后端。Moss 用 `MemoryArea<Backend>` 表示。 |
| PTE | page-table entry，页表项，把虚拟页映射到物理页并带权限。 |
| `pt` | page table，页表对象。 |
| `pn` | page number，文件内以 4 KiB 为单位的页号。 |
| `flags` | mapping flags，VMA 允许的权限。 |
| `access_flags` | 本次异常所需权限，例如 `WRITE | USER`。 |
| COW | copy-on-write，读时共享，第一次写时复制。 |
| folio | 当前实现中一个含 1～16 个连续 4 KiB 子页的文件缓存对象。 |
| fault-around | 修复当前故障页时，顺便映射附近已经有效的缓存页。 |
| readahead | 预测未来访问并提前把数据送入页缓存；当前另有异步工作队列。 |
| pin | 持有引用，防止某个缓存页仍被 PTE 使用时释放物理帧。 |
| TLB | CPU 对页表翻译结果的缓存。改 PTE 后可能需要失效旧翻译。 |
| stale | 计划执行期间映射已变化，计划的观察前提过期。 |

### 2. Rust 语法词

| 写法 | 含义 |
| --- | --- |
| `pub` / `pub(crate)` | 分别向 crate 外公开、只在当前 crate 内公开。 |
| `fn` | 定义函数；函数名通常用动词开头。 |
| `struct` / `enum` | 定义结构体/枚举。枚举一次只能是其中一个变体。 |
| `impl T` | 为类型 `T` 实现方法。 |
| `self` / `Self` | 当前对象 / 当前类型。 |
| `&T` / `&mut T` | 共享借用 / 独占可修改借用。 |
| `Arc<T>` | 原子引用计数共享所有权，可跨线程持有。 |
| `Weak<T>` | 不增加强引用计数的弱引用；使用前需要 `upgrade()`。 |
| `Option<T>` | `Some(T)` 或 `None`，表示可能存在。 |
| `Result<T, E>` | `Ok(T)` 或 `Err(E)`，表示成功或错误。 |
| `?` | 遇到 `Err`/`None` 就提前返回，否则取出成功值。 |
| `let` | 创建局部绑定；`let mut` 表示绑定可以修改。 |
| `let Some(x) = y else` | 若 `y` 是 `Some` 就取出 `x`，否则执行 `else`。 |
| `if let` | 只关心一种模式的条件分支。 |
| `match` | 对枚举或模式做完整分支。 |
| `=>` | 模式对应的表达式。 |
| `|` / `|=` | 位或 / 位或后写回，权限位组合用它。闭包参数两侧的 `|x|` 是另一种语法。 |
| `!` | 布尔取反；在宏名后如 `warn!` 表示调用宏。 |
| `::` | 类型、模块或枚举变体的路径分隔，例如 `FaultCommit::Stale`。 |
| `.` | 调用对象方法或访问字段。 |
| `as` | 显式数值或指针类型转换。 |
| `_name` | 前导下划线表示有意暂时不用该参数，避免编译器警告。 |
| `..` | 忽略结构体其余字段；在范围中表示半开区间。 |
| `||` / `&&` | 逻辑或 / 逻辑与，分别短路求值。 |
| `|| { ... }` | 无参数闭包；与逻辑或可由上下文区分。 |
| `move` | 把闭包捕获的所有权移入闭包。 |
| `unsafe` | 编译器无法替程序证明内存/指令安全，调用者在此承担不变量。 |
| `drop(x)` | 立刻结束 `x` 的生命周期，常用于明确提前释放锁。 |

命名约定也很重要：

- `prepare_*` 只观察并准备，不应完成不可回滚的提交。
- `execute` 执行计划里的慢工作。
- `commit_*` 重新验证后发布结果。
- `get_or_publish` 表示“已有就取；没有就尝试成为创建者”。
- `uptodate` 沿用内核术语，表示缓存内容与应读到的文件内容一致且可用。
- `invalidate` 表示使旧缓存或映射失效。

## 三、从硬件异常到 `ReturnReason::PageFault`

### 1. RISC-V 用户态入口

当前代码在 `vendor/axcpu/src/riscv/uspace.rs:61-90`：

```rust
let scause = scause::read();
let ret = if let Ok(cause) = scause.cause().try_into::<I, E>() {
    let stval = stval::read();
    match cause {
        Trap::Interrupt(_) => {
            handle_trap!(IRQ, scause.bits());
            ReturnReason::Interrupt
        }
        Trap::Exception(E::UserEnvCall) => {
            self.sepc += 4;
            ReturnReason::Syscall
        }
        Trap::Exception(E::LoadPageFault) => {
            ReturnReason::PageFault(va!(stval), PageFaultFlags::READ | PageFaultFlags::USER)
        }
        Trap::Exception(E::StorePageFault) => ReturnReason::PageFault(
            va!(stval),
            PageFaultFlags::WRITE | PageFaultFlags::USER,
        ),
        Trap::Exception(E::InstructionPageFault) => ReturnReason::PageFault(
            va!(stval),
            PageFaultFlags::EXECUTE | PageFaultFlags::USER,
        ),
        Trap::Exception(e) => ReturnReason::Exception(ExceptionInfo { e, stval }),
    }
} else {
    ReturnReason::Unknown
};
crate::asm::enable_irqs();
ret
```

逐行解释：

- `let scause = scause::read();`：`scause` 是 RISC-V supervisor cause CSR；模块名和局部
  变量同名。`read` 读出硬件记录的异常/中断原因。
- `let ret = if let Ok(cause) = ...`：最终返回值绑定为 `ret`。`cause()` 取原因位，
  `try_into::<I, E>()` 尝试解成中断枚举 `I` 或异常枚举 `E`；成功时取出 `cause`。
- `let stval = stval::read();`：`stval` 是 trap value CSR；页异常时保存故障虚拟地址。
- `match cause`：按硬件原因分派。
- `Trap::Interrupt(_)`：任意中断；`_` 表示这里不需要具体中断值。
- `handle_trap!(IRQ, scause.bits())`：把原始原因位交给已注册 IRQ 处理器。
- `ReturnReason::Interrupt`：告诉上层本次返回用户态的原因是中断。
- `E::UserEnvCall`：用户执行 `ecall`；`self.sepc += 4` 跳过 4 字节系统调用指令，否则
  返回用户态会再次执行它；随后返回 `Syscall`。
- `E::LoadPageFault`：读/加载缺页；`va!(stval)` 把整数包装成 `VirtAddr`；
  `READ | USER` 同时说明访问类型和特权来源。
- `E::StorePageFault`：写/存储缺页，生成 `WRITE | USER`。
- `E::InstructionPageFault`：取指缺页，生成 `EXECUTE | USER`。
- `E::Exception(e)`：其他已知异常，把具体异常 `e` 和附加值 `stval` 交给通用异常路径。
- `else ReturnReason::Unknown`：原因位无法解码时返回未知。
- `enable_irqs()`：用户运行函数进入时关闭了中断；回到 Rust 上层前恢复。
- 最后一行 `ret` 没有分号，所以是函数返回表达式。

### 2. LoongArch 用户态入口

`vendor/axcpu/src/loongarch64/uspace.rs:48-79` 做同一件事：

```rust
let estat = estat::read();
let badv = badv::read().vaddr();
let badi = badi::read().inst();

let ret = match estat.cause() {
    Trap::Interrupt(_) => { /* IRQ */ }
    Trap::Exception(Exception::Syscall) => { /* syscall */ }
    Trap::Exception(Exception::LoadPageFault)
    | Trap::Exception(Exception::PageNonReadableFault) => {
        ReturnReason::PageFault(va!(badv), PageFaultFlags::READ | PageFaultFlags::USER)
    }
    Trap::Exception(Exception::StorePageFault)
    | Trap::Exception(Exception::PageModifyFault) => {
        ReturnReason::PageFault(va!(badv), PageFaultFlags::WRITE | PageFaultFlags::USER)
    }
    Trap::Exception(Exception::FetchPageFault)
    | Trap::Exception(Exception::PageNonExecutableFault) => {
        ReturnReason::PageFault(va!(badv), PageFaultFlags::EXECUTE | PageFaultFlags::USER)
    }
    Trap::Exception(e) => ReturnReason::Exception(ExceptionInfo { e, badv, badi }),
    _ => ReturnReason::Unknown,
};
```

- `estat` 是异常状态寄存器；`cause()` 取异常原因。
- `badv` 是 bad virtual address，`vaddr()` 取故障虚拟地址。
- `badi` 是 bad instruction，`inst()` 取导致异常的指令编码，供其他异常诊断使用。
- LoongArch 把“不存在”和“存在但不可读/不可写/不可执行”分成不同异常，所以每类用
  `|` 模式合并：
  - `LoadPageFault | PageNonReadableFault` 都是 `READ`。
  - `StorePageFault | PageModifyFault` 都是 `WRITE`。
  - `FetchPageFault | PageNonExecutableFault` 都是 `EXECUTE`。
- 此处模式之间的单个 `|` 是“或模式”，不是权限位或；右侧
  `PageFaultFlags::READ | PageFaultFlags::USER` 才是位或。

`PageFaultFlags` 并非另一套类型。`vendor/axcpu/src/trap.rs:8` 用
`pub use page_table_entry::MappingFlags as PageFaultFlags;` 给 `MappingFlags` 起了一个
语义别名，所以异常访问位和 VMA/PTE 权限位能直接用 `contains` 比较。

## 四、用户任务总入口

核心是 `kernel/src/task/user.rs:66-124`。先看结构：

```rust
match reason {
    ReturnReason::Syscall => handle_syscall(&mut uctx),
    ReturnReason::PageFault(addr, flags) => {
        let fault_signal = {
            let aspace_ref = thr.proc_data.aspace();
            if is_mmap_sigbus_fault(...) {
                Some(Signo::SIGBUS)
            } else if AddrSpace::handle_page_fault_concurrent(&aspace_ref, addr, flags) {
                None
            } else {
                let mut aspace = aspace_ref.lock();
                if try_growdown_fault(...) {
                    None
                } else {
                    let signo = if aspace.is_accessible_fault(addr, flags) {
                        Signo::SIGBUS
                    } else {
                        Signo::SIGSEGV
                    };
                    /* 记录地址、指令和栈映射 */
                    Some(signo)
                }
            }
        };
        if let Some(signo) = fault_signal {
            /* 记录并投递致命信号 */
        }
    }
    /* 其他 ReturnReason */
}
```

逐行、逐标识符解释：

- `match reason`：`reason` 是 `uctx.run()` 返回的 `ReturnReason`，这里做完整分类。
- `ReturnReason::Syscall => handle_syscall(&mut uctx)`：系统调用分支把可修改的用户上下文
  传给系统调用处理器，以便读取参数和写回返回值。
- `ReturnReason::PageFault(addr, flags)`：模式同时取出 `addr`（故障 VA）和 `flags`
  （本次访问位）。
- `let fault_signal = { ... };`：这个代码块最终产生 `Option<Signo>`。`None` 表示缺页已经
  修复；`Some` 表示需要投递信号。
- `let aspace_ref = thr.proc_data.aspace();`：
  - `thr` 是当前线程。
  - `proc_data` 是线程所属进程数据。
  - `aspace()` 返回共享的 `Arc<AddrSpaceLock>`。
  - 名字加 `_ref` 强调这是共享引用，不是已经取得的锁 guard。
- `is_mmap_sigbus_fault(pid, addr, flags)`：先查 mmap 层显式记录的 `SIGBUS_AREAS`。当前
  实现只在 `flags` 包含 `WRITE` 且地址落入该进程记录区间时返回真。
- `Some(Signo::SIGBUS)`：无需尝试填页，直接选择总线错误。
- `else if AddrSpace::handle_page_fault_concurrent(...)`：调用当前并发缺页主路径。返回
  `true` 表示可以重试原指令。
- `None`：成功时不投递信号。
- `let mut aspace = aspace_ref.lock();`：主路径失败后取得地址空间**写锁**。`mut` 是因为
  grow-down 会修改 VMA/页表。
- `try_growdown_fault(pid, &mut aspace, addr, flags)`：如果地址位于记录的向下增长区间，
  为对齐后的 4 KiB 页建立匿名 COW 区域。成功也返回 `None`。
- `aspace.is_accessible_fault(addr, flags)`：重新查 VMA；如果地址属于一个包含本次访问
  权限的区域，说明映射合法但后端填页失败，代码选择 `SIGBUS`；否则选 `SIGSEGV`。
- 三次 `log_user_fault_mapping`：分别记录故障地址、当前指令地址 `uctx.ip()` 和栈指针
  `uctx.sp()` 所在映射，仅用于诊断，不参与修复。
- `Some(signo)`：把选择的信号带出代码块。
- `if let Some(signo) = fault_signal`：只有失败时进入。
- `warn!`：打印进程、信号、地址、访问位、IP、SP。
- `raise_signal_fatal(SignalInfo::new_kernel(signo))`：
  - `SignalInfo::new_kernel` 标记信号由内核生成。
  - `raise_signal_fatal` 向当前线程投递致命信号。
- `.expect("Failed to send fault signal")`：如果连信号投递都失败，内核在此 panic，而不是
  静默继续执行错误地址。

## 五、内核访问用户内存时的入口

`kernel/src/mm/access.rs:46-69` 在正式复制前预检查整个区域：

```rust
fn check_region(start: VirtAddr, layout: Layout, access_flags: MappingFlags) -> AxResult<()> {
    let align = layout.align();
    if start.as_usize() & (align - 1) != 0 {
        return Err(AxError::BadAddress);
    }
    let curr = current();
    let aspace_ref = curr.as_thread().proc_data.aspace();
    if !aspace_ref.read().can_access_range(start, layout.size(), access_flags) {
        return Err(AxError::BadAddress);
    }
    let page_start = start.align_down_4k();
    let page_end = (start + layout.size()).align_up_4k();
    for page in PageIter4K::new(page_start, page_end).ok_or(AxError::BadAddress)? {
        if !AddrSpace::populate_page_concurrent(&aspace_ref, page, access_flags) {
            return Err(AxError::BadAddress);
        }
    }
    Ok(())
}
```

- `layout` 同时携带对象大小和对齐要求。
- `start.as_usize() & (align - 1)` 取地址在二次幂对齐中的低位；非零即未对齐。
- `can_access_range` 在读锁下确认整个范围被 VMA 覆盖且权限足够。
- `align_down_4k` 和 `align_up_4k` 把任意字节范围扩到覆盖它的完整页范围。
- `PageIter4K::new` 创建逐 4 KiB 页迭代器；构造失败转成 `BadAddress`。
- `populate_page_concurrent` 与用户异常走同一内部机制，但传
  `flush_existing = false`：这是主动预填，不是本 CPU 已经缓存无效翻译后重试异常。

若预检查和实际复制之间仍发生缺页，`access.rs:272-287` 注册的内核 trap handler 兜底：

```rust
#[register_trap_handler(PAGE_FAULT)]
fn handle_page_fault(vaddr: VirtAddr, access_flags: MappingFlags) -> bool {
    let curr = current();
    let Some(thr) = curr.try_as_thread() else { return false; };
    if unlikely(!thr.is_accessing_user_memory()) { return false; }
    let aspace = thr.proc_data.aspace();
    AddrSpace::handle_page_fault_concurrent(&aspace, vaddr, access_flags)
}
```

- 属性宏把函数放进 `PAGE_FAULT` 分布式 handler 列表。
- `try_as_thread` 防止在非普通用户线程上下文误处理。
- `is_accessing_user_memory` 只在受保护的用户复制区间为真；否则内核缺页不应被当成
  用户懒分配掩盖。
- `unlikely` 是分支预测提示，不改变语义。

架构内核 trap 再包了一层。RISC-V 从 `stval` 取地址后调用注册的 `PAGE_FAULT`
handler；若没人处理且 exception fixup 也失败才 panic。LoongArch 还创建
`KernelPageFaultIrqGuard`：异常入口默认清中断使能，但当前缺页路径可能睡眠、被抢占或
参加跨 CPU TLB shootdown，所以 guard 按保存的 `tf.prmd` 暂时恢复异常前的中断状态，
调用 handler 后再关闭中断，交给汇编返回路径。这个 guard 只围绕内核态 trap；用户态
`UserContext::run` 返回 Rust 循环后走的是前文的 `ReturnReason` 路径。

## 六、`FaultPreparation` 和后端分派

`kernel/src/mm/aspace/backend/mod.rs:24-40`：

```rust
pub(crate) enum FaultPreparation {
    Handled,
    Cow(CowFaultPlan),
    File(FileFaultPlan),
    Fallback,
}

pub(crate) enum FaultCommit {
    Handled,
    Stale,
}
```

- `FaultPreparation` 是短锁阶段的输出。
- `Handled`：当前 PTE 已满足访问，不需要计划。
- `Cow(CowFaultPlan)`：枚举变体内携带一份 COW 计划。
- `File(FileFaultPlan)`：携带一份共享文件计划。
- `Fallback`：当前快路径不覆盖，交给旧的独占路径。
- `FaultCommit::Handled`：计划已提交，或竞争者已经完成等价映射。
- `FaultCommit::Stale`：计划前提过期，不能使用这份结果。

`Backend::prepare_fault` 的分派是：

```rust
match self {
    Self::Cow(backend) => backend.prepare_fault(...),
    Self::File(backend) => backend.prepare_fault(...),
    _ => FaultPreparation::Fallback,
}
```

- `self` 是当前 VMA 的 `Backend` 枚举。
- `Self::Cow(backend)` / `Self::File(backend)` 取出具体后端并调用自己的实现。
- `_` 覆盖 `Linear`、`Shared` 以及未来其他变体，所以它们当前不能走两阶段快路径。

两个 commit 分派还检查枚举种类。若 prepare 后 VMA 已换成另一种 Backend，分派返回
`Stale`，不会把 Cow 计划交给 File 后端或反过来。

## 七、地址空间的两阶段主函数

完整关键函数在 `kernel/src/mm/aspace/mod.rs:587-694`。

### 0. `AddrSpaceLock` 本身保护什么

当前锁不是自旋等待的普通 `spin::RwLock`。`kernel/src/mm/aspace/mod.rs:20-126` 实现了
一个会让等待任务睡眠、并偏向已经排队 writer 的读写锁：

```rust
const WRITER_LOCKED: usize = 1;
const ONE_READER: usize = 2;

pub(crate) struct RawAddrSpaceRwLock {
    state: AtomicUsize,
    waiting_writers: AtomicUsize,
    event: Event,
}
```

- `state` 的最低位 `1` 表示 writer 独占；每个 reader 给数值加 `2`，所以 reader 数和
  writer 位不会混在一起。
- `waiting_writers` 记录已经排队的写者。
- `event` 用来让拿不到锁的任务睡眠并在解锁时唤醒，避免一直占 CPU 自旋。

`lock_shared` 的循环先 `try_lock_shared()`，失败后注册 `listener`，再试一次，仍失败才
`block_on(waiter)`。注册后重试是为了关闭“第一次失败与注册监听之间恰好发生解锁”的
丢失唤醒窗口。

`try_lock_shared` 逐行做：

1. 用 `Acquire` 读 `waiting_writers`；非零就拒绝新 reader，让排队 writer 不被持续
   涌入的缺页饿死。
2. 读 `state`。
3. 若 `state & WRITER_LOCKED != 0`，已有 writer，返回假。
4. `compare_exchange_weak(state, state + ONE_READER, Acquire, Relaxed)` 原子增加 reader。
5. CAS 成功返回真；失败把别人刚写入的值放进 `next`，更新本地 `state` 后重试。

`unlock_shared` 用 `fetch_sub(2, Release)` 删除一个 reader。`previous == ONE_READER`
表示自己是最后一个 reader，于是 `notify` 等待者。`debug_assert` 只在调试检查引用状态，
release 版不能把它当运行时错误处理。

`lock_exclusive`：

1. 先把 `waiting_writers` 加一，阻止新 reader 插队。
2. 只有 `state == 0` 时，CAS 才能把它改成 `WRITER_LOCKED`。
3. 失败时同样采用“注册 listener、再试、再睡眠”。
4. 成功后把 `waiting_writers` 减一。

`unlock_exclusive` 把 `state` 以 `Release` 写回零并通知等待者。

`unsafe impl lock_api::RawRwLock` 中的 `unsafe` 表示这份实现必须自行保证
lock/unlock 配对、内存序和 guard 互斥不变量。上层：

```rust
aspace.read()  // BlockingRwLockReadGuard，可与其他缺页并存
aspace.lock()  // BlockingRwLockWriteGuard，修改 VMA 的独占侧
```

所以“读锁”并不表示 PTE 完全不可修改。当前设计让 VMA 结构在读锁期间保持稳定，而叶
PTE 只能通过后文的原子 API 修改；传统页表游标修改仍需要写锁。

### 1. 函数签名

```rust
fn handle_page_fault_concurrent_inner(
    aspace: &Arc<AddrSpaceLock>,
    vaddr: VirtAddr,
    access_flags: PageFaultFlags,
    flush_existing: bool,
) -> bool {
```

- `handle_page_fault`：处理页异常。
- `concurrent`：允许多个缺页在短读锁阶段和 PTE 提交阶段并发。
- `inner`：供用户异常和内核主动 populate 两个公开包装共同调用。
- `aspace: &Arc<AddrSpaceLock>`：借用一个共享拥有的地址空间锁对象。
- `vaddr`：故障地址，后端会按页对齐。
- `access_flags`：本次需要的权限。
- `flush_existing`：若发现已有映射，是否仍刷新本地 TLB。真实异常为 `true`，主动预填
  为 `false`。
- `-> bool`：只能表达“处理成功/失败”；详细 I/O 错误在内部日志中记录。

### 2. prepare 段

```rust
let preparation = {
    let guard = aspace.read();
    if !guard.va_range.contains(vaddr) {
        return false;
    }
    let Some(area) = guard.areas.find(vaddr) else {
        return false;
    };
    if !area.flags().contains(access_flags) {
        return false;
    }
    area.backend().prepare_fault(
        vaddr,
        area.va_range(),
        area.flags(),
        access_flags,
        &guard.pt,
    )
};
```

- `preparation` 保存 `FaultPreparation`。
- 外层 `{}` 限定 `guard` 生命周期；代码块结束时自动释放读锁。
- `aspace.read()` 取得共享读 guard。其他缺页也能拿读锁，VMA 修改者不能。
- `va_range.contains(vaddr)` 检查地址是否位于整个用户地址空间范围。
- `guard.areas.find(vaddr)` 在 VMA 集合中找包含地址的 area。
- `let Some(area) ... else` 把“没有 VMA”直接变成失败。
- `area.flags().contains(access_flags)` 要求 VMA 的所有权限位包含本次访问位。因为用户
  VMA 通常含 `USER`，所以也校验了来源位。
- `area.backend()` 取该 VMA 的数据来源。
- `area.va_range()` 把当前 VMA 范围复制进计划，供无锁执行后检查边界。
- `&guard.pt` 共享借用页表；prepare 只查询，不修改。
- 最后一行无分号，它的返回值成为 `preparation`。

### 3. 已处理和回退分支

```rust
match preparation {
    FaultPreparation::Handled => {
        if flush_existing {
            axhal::asm::flush_tlb(Some(vaddr));
            synchronize_executable_fault(access_flags);
        }
        true
    }
    FaultPreparation::Fallback => aspace.lock().handle_page_fault(vaddr, access_flags),
```

- `match preparation` 消耗计划枚举。
- `Handled` 可能表示本 CPU 等锁时另一 CPU 已安装映射。
- `flush_tlb(Some(vaddr))` 只失效当前地址的本地翻译，不是全 TLB 刷新。
- `synchronize_executable_fault` 只在 `EXECUTE` 时执行架构屏障。
- 分支最后的 `true` 是整个函数的成功值。
- `Fallback` 用 `aspace.lock()` 取写 guard，再自动借用成 `&mut AddrSpace` 调旧函数；
  这个表达式直接返回旧函数的 bool。

### 4. COW execute 段

```rust
FaultPreparation::Cow(mut plan) => {
    if let Err(err) = plan.execute() {
        warn!("Failed to prepare private page for {vaddr:?}: {err}");
        return false;
    }
```

- `mut plan`：`execute` 要把新帧或 folio 引用写进计划。
- `if let Err(err)`：只展开失败分支；成功值是空元组 `()`，无需绑定。
- 此时 prepare 代码块的 `guard` 已释放，所以分配、复制、文件读不持有 AddrSpace 锁。
- 日志里的 `private page` 对应 Cow/private 后端；`{vaddr:?}` 和 `{err}` 是格式化捕获。

### 5. COW revalidate/commit 段

```rust
let committed = {
    let guard = aspace.read();
    let Self { areas, pt, .. } = &*guard;
    let Some(area) = areas.find(vaddr) else {
        return false;
    };
    if !area.flags().contains(access_flags) {
        return false;
    }
    let flags = area.flags();
    area.backend()
        .commit_fault(&mut plan, area.va_range(), flags, access_flags, pt)
};
```

- 重新拿读锁是**重新验证**，不是默认相信 prepare 时的 area 还存在。
- `&*guard`：先解引用 guard 得到 `AddrSpace`，再共享借用。
- `let Self { areas, pt, .. }`：结构体解构只取 `areas` 和 `pt`；`..` 忽略 `va_range`。
  这样 Rust 能同时借用两个字段。
- 再次 `find(vaddr)` 和 `contains(access_flags)`：处理 execute 期间发生的 unmap/protect。
- `flags` 是**现在**的 VMA flags，不复用旧值。
- `commit_fault(&mut plan, ...)` 可以拿走计划中的 frame，因此计划仍是可修改借用。
- `pt` 是共享页表引用；真正写 PTE 的函数内部使用原子操作。

结果处理：

```rust
match committed {
    Ok(FaultCommit::Handled) => {
        flush_tlb(Some(vaddr));
        synchronize_executable_fault(access_flags);
        true
    }
    Ok(FaultCommit::Stale) => aspace.lock().handle_page_fault(vaddr, access_flags),
    Err(err) => {
        warn!(...);
        false
    }
}
```

- 外层 `Ok/Err` 是操作本身是否出错；内层 `Handled/Stale` 是并发状态。
- commit 成功总是刷新本地故障地址。这里没有 `flush_existing` 判断，因为 Cow 计划来自
  真实缺失/权限异常，主动 populate 若走到这里也新装了 PTE。
- `Stale` 取得写锁回退，优先保证正确性；计划析构时会释放没被提交的新帧。
- `Err` 不再回退，因为它表示明确错误而非普通竞争。

### 6. File 分支

`FaultPreparation::File(mut plan)` 与 COW 结构相同，差异只有：

- 错误日志写 `file page`。
- 调用 `plan.execute()` 取得 `FolioPageRef`。
- commit 调 `commit_file_fault`，最终分派到 `FileBackend::commit_fault`。

这份重复代码让两种计划类型保持静态类型，不需要把不同计划装入动态 trait object。

## 八、旧的独占回退路径

`kernel/src/mm/aspace/mod.rs:524-564`：

```rust
pub fn handle_page_fault(&mut self, vaddr: VirtAddr, access_flags: PageFaultFlags) -> bool {
    if !self.va_range.contains(vaddr) {
        return false;
    }
    if let Some(area) = self.areas.find(vaddr) {
        let flags = area.flags();
        if flags.contains(access_flags) {
            let page_size = area.backend().page_size();
            let populate_result = area.backend().populate(
                VirtAddrRange::from_start_size(vaddr.align_down(page_size), page_size as _),
                flags,
                access_flags,
                &mut self.pt.modify(),
            );
            return match populate_result {
                Ok((n, callback)) => { /* 完成回调、检查 n、同步 */ }
                Err(err) => { /* 日志并返回 false */ }
            };
        }
    }
    false
}
```

逐行含义：

- `&mut self` 是关键：调用期间独占整个 `AddrSpace`。
- 两次范围/权限检查与快路径相同，但写锁已经由调用者持有。
- `backend.page_size()` 允许旧路径处理 4 KiB 之外的后端页大小。
- `vaddr.align_down(page_size)` 找到后端页起点。
- `VirtAddrRange::from_start_size` 只要求 backend populate 当前一页。
- `self.pt.modify()` 创建传统可修改页表游标；它不是并发 PTE CAS API。
- `Ok((n, callback))`：
  - `n` 是现在满足访问的页数。
  - `callback` 是可选的 `FnOnce(&mut AddrSpace)`，给需要填页后修改地址空间的后端使用。
- `if let Some(cb) = callback { cb(self); }`：存在才调用，`FnOnce` 只能消费一次。
- `n == 0`：后端没报错但也没解决任何页，记录警告并失败。
- `n > 0`：刷新本地故障地址 TLB，执行必要的取指屏障，返回真。
- `Err(err)`：记录 backend populate 的明确错误并返回假。

它仍有存在价值：当前并发快路径没有覆盖所有后端、页大小和罕见竞争状态。但它也正是
不能在慢 I/O 中获得跨页并发的路径。

## 九、共享文件缺页逐行读

### 0. 映射 pin 为什么按地址分片

`FilePageRefs` 内含 64 个
`Mutex<BTreeMap<VirtAddr, FolioPageRef>>`。`shard(vaddr)` 用
`(vaddr / 4096) % 64` 选择一个 map：

- `insert` 在 PTE 发布成功后记录 `VA → folio 子页 pin`。
- `get` 克隆 pin，调用者离开 map 锁后仍能安全使用页。
- `remove` 在 unmap、evict 或 COW 替换时删除 pin。

因此不同虚拟页通常只竞争各自 shard，而不是共享一把整个进程的 mapped-file-page 锁。
64 是固定分片数，不代表系统只能映射 64 页；每个 shard 的 `BTreeMap` 可以容纳任意多
项。

文件页缓存回收/截断时通过 evict listener 通知 `FileBackendInner`。listener 先
`aspace.try_lock()` 尝试地址空间写锁，成功就核对 backend 身份、pin PA 和当前 PTE 后
unmap；拿不到时把 `(pn, page)` 放进 `pending_evictions`，只启动一个延后 worker 再取
写锁处理。它不会在页缓存回调中无限自旋等地址空间锁。

### 1. `FileFaultPlan`

`kernel/src/mm/aspace/backend/file.rs:188-207`：

```rust
pub(crate) struct FileFaultPlan {
    identity: Arc<FileBackendInner>,
    vaddr: VirtAddr,
    pn: u32,
    page: Option<FolioPageRef>,
}
```

- `identity` 既固定后端对象生命周期，也用于 commit 时用 `Arc::ptr_eq` 检查是否还是同
  一个实例。
- `vaddr` 是对齐后的虚拟页地址。
- `pn` 是加上 mmap 文件偏移后的文件页号。
- `page` 在 prepare 时是 `None`，execute 成功后是 `Some(FolioPageRef)`。

`execute`：

```rust
let offset = u64::from(self.pn) * PAGE_SIZE_4K as u64;
if offset >= self.identity.cache.len()? {
    return Err(AxError::Io);
}
self.page = Some(self.identity.cache.fault_page_ref(self.pn)?);
Ok(())
```

- `u64::from(self.pn)` 无损扩大页号，避免先用 32 位乘法溢出。
- 乘 `PAGE_SIZE_4K` 得文件字节偏移。
- `cache.len()?` 读取当前文件长度；长度查询失败原样返回。
- 起始偏移已在 EOF 之外时返回 `Io`，上层最终按可访问映射失败处理为 `SIGBUS`。
- `fault_page_ref` demand-load 包含该页的 folio，并返回稳定子页引用。
- `Some(...)` 写回计划。
- `Ok(())` 的 `()` 表示只报告完成，没有额外返回数据。

### 2. `prepare_fault`

```rust
let vaddr = vaddr.align_down(PAGE_SIZE_4K);
match pt.query(vaddr) {
    Ok((_paddr, page_flags, PageSize::Size4K))
        if page_flags.contains(access_flags) => FaultPreparation::Handled,
    Ok(_) => FaultPreparation::Fallback,
    Err(PagingError::NotMapped) => {
        let pn = ((vaddr - self.0.start) / PAGE_SIZE_4K) as u32 + self.0.offset_page;
        FaultPreparation::File(FileFaultPlan {
            identity: self.0.clone(),
            vaddr,
            pn,
            page: None,
        })
    }
    Err(_) => FaultPreparation::Fallback,
}
```

- 第一行把字节地址向下对齐到 4 KiB PTE。
- `pt.query` 返回 `(物理地址, PTE flags, 页大小)` 或 `PagingError`。
- `_paddr` 前导下划线表示模式需要占位但这里不使用物理地址。
- 守卫 `if page_flags.contains(access_flags)` 只有权限已足够才 `Handled`。
- 任何“已经映射但不满足上述精确条件”的状态回退；共享文件写权限升级由旧 populate
  路径处理。
- `NotMapped` 才创建新文件计划。
- `vaddr - self.0.start` 是该 VMA 内字节偏移；除 4 KiB 得 VMA 内页号。
- `offset_page` 是 mmap 文件偏移换算成的起始页号。
- `self.0.clone()` 克隆 `Arc`，不是复制整个 cache/backend。
- 其他页表错误不能当普通未映射处理，所以回退。

### 3. `commit_fault`

首先验证后端身份：

```rust
if !Arc::ptr_eq(&self.0, &plan.identity) {
    return Ok(FaultCommit::Stale);
}
```

`ptr_eq` 比较两个 `Arc` 是否指向同一分配对象，不比较字段值。VMA 即使换成内容相似的
新 FileBackend，也不能提交旧计划。

查询 PTE 后有三类结果：

- 已有 4 KiB PTE 且权限足够：竞争者已经完成，返回 `Handled`。
- 已有其他状态：返回 `Stale`。
- `NotMapped`：尝试提交计划页。

未映射分支逐行：

```rust
let page = plan.page.as_ref().ok_or(AxError::BadAddress)?;
let mapping_page = page.clone();
let write_fault = access_flags.contains(MappingFlags::WRITE);
let map_flags = if self.0.cache.in_memory() || write_fault {
    flags
} else {
    flags - MappingFlags::WRITE
};
```

- `as_ref()` 从 `Option<FolioPageRef>` 取得借用，不立即把 page 移出计划。
- `ok_or` 把异常的 `None` 转成错误。
- `mapping_page` 是准备放入 `mapped_pages` 的额外 pin。
- `write_fault` 明确记录本次是不是写。
- tmpfs 或写缺页使用完整 VMA 权限；磁盘文件的普通读缺页先去掉 `WRITE`。
- `flags - MappingFlags::WRITE` 是 bitflags 的删除位运算，不是整数含义的减法。

真正提交：

```rust
let committed = self.0.cache.with_valid_page_ref(
    plan.pn,
    page,
    write_fault,
    || {
        match pt.map_4k_concurrent(plan.vaddr, page.paddr(), map_flags) {
            Ok(()) => {
                self.0.mapped_pages.insert(plan.vaddr, mapping_page);
                Ok(true)
            }
            Err(PagingError::AlreadyMapped) => Ok(false),
            Err(err) => Err(AxError::from(err)),
        }
    },
);
```

- `with_valid_page_ref` 在 per-inode invalidate 读锁和 folio 状态锁保护下确认页仍属于当前
  cache、仍 `uptodate`、未 truncate；`write_fault` 为真时对磁盘文件标脏。
- `||` 是无参数 commit 闭包。所有存储 I/O 已在进入它之前完成。
- `page.paddr()` 取 folio 子页物理地址。
- `map_4k_concurrent` 用叶 PTE CAS 安装。
- PTE 成功后立刻把对应 pin 插入按虚拟地址分片的 `mapped_pages`。
- `Ok(true)` 表示本计划是胜者。
- `AlreadyMapped` 是正常竞争失败，转成 `Ok(false)`，不是 I/O 错误。
- 其他页表错误转成 `AxError`。

结果：

```rust
match committed {
    Some(Ok(true)) => {
        plan.page = None;
        Ok(FaultCommit::Handled)
    }
    Some(Ok(false)) | None => Ok(FaultCommit::Stale),
    Some(Err(err)) => Err(err),
}
```

- 外层 `Option` 来自 cache 有效性检查；`None` 表示页已失效。
- `Some(Ok(true))` 时把 `plan.page` 清空。映射 pin 已转移到 `mapped_pages`；计划析构
  不再拥有它。
- `Some(Ok(false))` 是 PTE 竞争失败；`None` 是 folio 竞争失效，均返回 `Stale`。
- `Some(Err(err))` 是 commit 闭包的真正错误。

### 4. File 的旧 `populate` 回退到底做什么

若 prepare 发现已有但权限不合适的 PTE，或计划变 stale，就会进入
`FileBackend::populate`：

1. 根据 range 起点和 `offset_page` 算第一个文件页号。
2. 记录这是否是“恰好一个 4 KiB 页的读缺页”，供末尾顺序预读判断使用。
3. 每页先检查是否已经越过文件长度；越界返回 `Io`。
4. PTE 已存在且本次是写、PTE 却不可写时：
   - 查相应缓存页；
   - 磁盘文件先 `mark_dirty`，tmpfs 不需要；
   - 用传统页表游标 `remap` 成完整 VMA flags。
5. PTE 已存在且权限足够时只增加成功页数。
6. PTE 不存在时：
   - tmpfs 使用完整 flags，磁盘文件先去掉 `WRITE`；
   - `with_page_or_insert(pn, ...)` 当前传的 preferred order 是 0，所以 demand load 是
     单个 4 KiB 页；
   - 传统 `pt.map` 成功后插入 `mapped_pages` pin。
7. 至少成功一页且属于单页读 fault 时，调用 `maybe_readahead_after_fault`。

`maybe_readahead_after_fault` 用两个 `AtomicU32`：

- `last_fault_page.swap(pn)` 取得上一次 fault 页号；只有 `last + 1 == pn` 才判定为顺序
  访问。
- 从下一页开始取 4 页，即 `MMAP_FAULT_READAHEAD_PAGES = 4`。
- `next_readahead_page` 防止已覆盖的窗口重复排队。
- 最终调前文真正异步的 `cache.readahead(offset, count)`。

所以当前共享文件有两种不同策略：

- 两阶段并发 `FileFaultPlan`：demand-load 自然阶 folio，通常最多 64 KiB，没有在 commit
  后调用这段 4 页顺序预读。
- 旧 locked populate：demand-load 4 KiB，检测到连续 fault 后异步预读后续 4 页。

文档不能把后一种策略误写成所有 File 快路径都会执行。

## 十、COW 缺页逐行读

### 0. COW 匿名帧引用表

匿名 COW 页不能仅靠普通分配器判断是否被父子进程共享。当前代码用
`FRAME_TABLE_SHARDS = 64` 个 `SpinNoIrq<FrameTableRefCount>`，按
`(paddr / 4096) % 64` 选择引用表：

- `FrameTableRefCount.table` 是 `BTreeMap<PhysAddr, Arc<SpinNoIrq<FrameRefCnt>>>`。
- `init_frame` 为新匿名帧建立初始引用数 1。
- fork 映射同一帧时增加引用。
- COW prepare 临时再增加一份 pin。
- PTE 被替换、unmap 或计划析构时减少引用。
- 降到 0 时先从 table 删除，再交还物理分配器。这个顺序防止帧先被重新分配、旧表项
  后删除而误伤新拥有者。

这里的 `SpinNoIrq` 临界区只保护短引用计数或 map 操作，不能跨文件 I/O。64 个 shard
降低不同物理帧的引用计数竞争，但同一帧的计数仍必须串行更新。

### 1. 计划和源页

`CowFaultPlan` 的字段：

| 字段 | 含义 |
| --- | --- |
| `identity: Arc<()>` | 不承载数据的唯一身份 token，用指针相等判断后端是否还是原实例。 |
| `vaddr` | 对齐后的故障页 VA。 |
| `mapping_start` | 这份 CowBackend 的映射起点，用来换算文件偏移。 |
| `area_range` | prepare 时 VMA 范围，execute 计算 fault-around 边界时使用。 |
| `size` | 后端页大小；并发快路径当前只接受 4 KiB。 |
| `file` | 可选 `(FileBackend, 文件起始偏移, 可选文件结束偏移)`。`None` 表示匿名。 |
| `access_flags` | 本次读/写/执行权限需求。 |
| `frame` | execute 分配的新匿名物理帧，提交前由计划拥有。 |
| `file_pages` | fault-around 得到的 `(VA, FolioPageRef)` 列表。 |
| `source` | 已映射 COW 写缺页的源：匿名帧或文件缓存页。 |

`CowFaultSource::Anonymous(PhysAddr)` 通过额外引用计数固定匿名源帧；
`CowFaultSource::File(FolioPageRef)` 通过 `Arc<Folio>` 固定文件源页。`paddr()` 屏蔽两者
表示差异，给复制代码统一物理地址。

### 2. `prepare_fault`

```rust
if self.size != PageSize::Size4K {
    return FaultPreparation::Fallback;
}
let vaddr = vaddr.align_down(self.size as usize);
match pt.query(vaddr) { ... }
```

- 第一项明确限定：非 4 KiB COW 当前不走并发实现。
- 按后端页大小向下对齐。

若 PTE 已存在：

```rust
if page_size == self.size && page_flags.contains(access_flags) {
    FaultPreparation::Handled
} else if page_size == self.size
    && access_flags.contains(MappingFlags::WRITE)
    && !page_flags.contains(MappingFlags::WRITE)
{
    /* 固定源页并创建复制计划 */
} else {
    FaultPreparation::Fallback
}
```

- 页大小正确且权限足够：无需处理。
- 第二分支只接受“正确大小、当前是写、PTE 尚不可写”，即典型 COW 写保护异常。
- 其他权限错误或页大小异常不猜测，回退。

选择源页：

```rust
let file_page = self.file_pages.get(vaddr);
let source = if let Some(page) = file_page {
    if page.paddr() != paddr {
        return FaultPreparation::Fallback;
    }
    CowFaultSource::File(page)
} else {
    let Some(frame_ref) = frame_table(paddr).lock().get_frame_ref(paddr) else {
        return FaultPreparation::Fallback;
    };
    let mut frame_ref = frame_ref.lock();
    if frame_ref.0 == 1 {
        return FaultPreparation::Fallback;
    }
    let Some(count) = frame_ref.0.checked_add(1) else {
        return FaultPreparation::Fallback;
    };
    frame_ref.0 = count;
    CowFaultSource::Anonymous(paddr)
};
```

- 先查该 VA 是否由只读文件缓存页直接映射。
- 找到时必须检查 pin 的 PA 与 PTE PA 一致，否则内部状态不一致，回退。
- 未找到文件 pin 时查匿名 `FRAME_TABLE`。
- `frame_ref.0` 是引用数。等于 1 时旧路径可直接把唯一页升级为可写，无需复制，所以
  快路径回退给 `handle_cow_fault`。
- 多引用时 `checked_add(1)` 加一个**计划临时 pin**；溢出则回退。
- 这个额外引用保证释放 AddrSpace 锁后，源页仍不会被另一个 unmap 释放。

随后构造计划：`file: None` 是因为已有源页的写 COW 不再从原文件读取；
`source: Some(source)` 告诉 execute 走复制分支。

若 `pt.query` 返回 `NotMapped`，计划字段中：

- `file: self.file.clone()`：私有文件映射保留文件来源；匿名映射为 `None`。
- `source: None`：没有已映射源页。
- `frame: None`、`file_pages: Vec::new()`：由 execute 填充。

### 3. `execute` 的已有源页复制

```rust
if let Some(source) = &self.source {
    let frame = alloc_referenced_frame(false, self.size)?;
    self.frame = Some(frame);
    unsafe {
        core::ptr::copy_nonoverlapping(
            phys_to_virt(source.paddr()).as_ptr(),
            phys_to_virt(frame).as_mut_ptr(),
            self.size as _,
        );
    }
    return Ok(());
}
```

- `&self.source` 只借用源 pin，复制期间计划继续拥有它。
- `alloc_referenced_frame(false, ...)` 分配新帧并登记 COW 引用；`false` 表示马上完整覆盖，
  无需先清零。
- 先把 `frame` 放进计划，再做后续操作，保证提前返回时 `Drop` 能释放。
- `phys_to_virt` 把内核可访问的物理帧转换成直接映射 VA。
- `as_ptr` 是只读源指针，`as_mut_ptr` 是目标写指针。
- `copy_nonoverlapping` 要求源/目标不重叠；新分配帧满足这个不变量。
- 长度是 `self.size` 字节。

### 4. 未映射私有文件页

代码先计算三个量：

- `start`：映射起点若不按页对齐，文件内容在物理页内从哪个字节开始。
- `fault_file_start`：故障 VA 对应的文件字节偏移。
- `max_read`：不能越过文件映射结束，也不能越过本物理页剩余空间的最大读取长度。

只有以下条件同时成立才直接映射缓存页：

```rust
self.size == PageSize::Size4K
&& !self.access_flags.contains(MappingFlags::WRITE)
&& start == 0
&& max_read == self.size as usize
```

也就是：4 KiB 页、不是写缺页、文件数据覆盖完整物理页。部分页不能直接映射共享缓存，
因为页外字节需要保持该私有映射应有的零填充。

fault-around 边界：

```rust
let offset_in_folio = (fault_file_start % FILE_FAULT_AROUND_BYTES as u64) as usize;
let window_start = VirtAddr::from(self.vaddr.as_usize().saturating_sub(offset_in_folio));
let window_end = window_start + FILE_FAULT_AROUND_BYTES;
let first = window_start
    .max(self.area_range.start.align_up(page_size))
    .max(self.mapping_start.align_up(page_size));
let last = window_end.min(self.area_range.end.align_down(page_size));
```

- `% 64 KiB` 得故障文件偏移在自然 folio 内的位置。
- 从故障 VA 减去同样偏移，使虚拟窗口与文件 folio 对齐，而不是与 ASLR 后的 VA 盲目
  对齐。
- `saturating_sub` 防止无符号下溢。
- `first` 不能早于当前 VMA，也不能早于文件映射实际起点，并向上按页对齐。
- `last` 不能晚于 VMA 末端，并向下按页对齐。

装载与收集：

```rust
if let Some(page) = file.page_ref_at(fault_file_start)? {
    self.file_pages.push((self.vaddr, page));
    let mut addr = first;
    while addr < last {
        if addr != self.vaddr {
            let offset = mapping_file_start + (addr - mapping_start);
            let complete = file_end.is_none_or(|end| offset + page_size <= end);
            if complete && let Some(page) = file.cached_page_ref_at(offset) {
                self.file_pages.push((addr, page));
            }
        }
        addr += page_size;
    }
    return Ok(());
}
```

- `page_ref_at` 对 Cached 后端命中或 demand-load folio；Direct 后端返回 `None`。
- 故障页第一个放入列表，commit 会优先保证原异常得到修复。
- `while` 逐 4 KiB 遍历窗口。
- 邻页只调用 `cached_page_ref_at`，它绝不发 I/O；没准备好就跳过。
- `complete` 防止映射超出私有映射的文件有效区。
- 当前 demand-loaded folio 本身最大 64 KiB，所以通常邻页在第一次调用返回时已
  `uptodate`；这仍是同步完成后的缓存命中，不应称为异步邻页 I/O。

若不能直接映射缓存页：

```rust
let frame = alloc_referenced_frame(true, self.size)?;
self.frame = Some(frame);
let buf = slice::from_raw_parts_mut(...);
file.read_at(&mut &mut buf[start..start + max_read], fault_file_start)?;
```

- `true` 先把整页清零，保证文件未覆盖部分为零。
- `from_raw_parts_mut` 把新帧直接映射地址构造成可写 slice；其安全依赖帧有效、独占且
  长度正确。
- 双层 `&mut &mut` 是因为 `read_at` 接受实现 I/O buffer trait 的可变 reader/writer
  包装；最终只向目标区间读入 `max_read` 字节。

匿名且无源页时只执行 `alloc_referenced_frame(true, size)`，得到零页。

### 5. `CowFaultPlan::Drop`

```rust
if let Some(frame) = self.frame.take() {
    drop_referenced_frame(frame, self.size);
}
if let Some(CowFaultSource::Anonymous(frame)) = self.source.take() {
    drop_referenced_frame(frame, self.size);
}
```

- `take()` 用 `None` 替换 Option 并取出所有权。
- 尚未 commit 的新帧由计划释放。
- 匿名源页在 prepare 增加的临时引用也由计划释放。
- 文件源的 `FolioPageRef` 不需显式分支；字段正常析构会降低 `Arc` 计数。
- commit 成功会先把 `plan.frame = None`，因此不会错误释放已交给 PTE 的帧。

### 6. COW commit：已有源页

开头先检查：

```rust
if !Arc::ptr_eq(&self.identity, &plan.identity) || self.size != plan.size {
    return Ok(FaultCommit::Stale);
}
```

身份或页大小任一变化都使计划过期。

如果当前 PTE 已满足权限，竞争者已经解决，返回 `Handled`。否则有 `source` 时再次检查：

- 当前 PTE 页大小仍相同。
- PTE PA 仍等于被固定源页 PA。
- 当前请求仍是写。
- 当前 PTE 仍不可写。

任何一项不满足都返回 `Stale`。

文件源分支：

1. 只锁 `file_pages` 中该 VA 所在的 shard。
2. 确认 map 中仍有 pin 且 PA 一致。
3. `remap_4k_concurrent_if(vaddr, old_paddr, new_frame, flags)` 仅在 PTE 仍指向旧 PA 时 CAS。
4. 成功后删除旧文件 pin。

匿名源分支同样条件替换 PTE；成功后
`drop_referenced_frame(paddr, size)` 删除原 PTE 所拥有的一份引用。计划增加的临时引用
继续活到计划析构，保证 commit 完成全过程源页有效。

最后 `plan.frame = None` 把新帧所有权转给新 PTE，返回 `Handled`。

### 7. COW commit：未映射页

有 `file_pages` 时：

- `core::mem::take` 把整个向量移出计划。
- 按“故障页第一”顺序遍历。
- 每页重新检查仍在当前 `area_range` 内且当前 PTE 仍 `NotMapped`。
- 重新计算文件 offset，并用 `with_valid_page_ref` 确认缓存页仍有效。
- 只锁该 VA 的 file-page shard，再用 `map_4k_concurrent` 映射为
  `flags - WRITE`。
- PTE 成功后同一临界区插入 folio pin，避免 COW 线程看见“有 PTE、没 pin”的半发布
  状态。
- 可选邻页失败只跳过；故障页失败返回 `Stale`。
- 最后重新查询故障页 PTE，只有页大小和权限满足才返回 `Handled`。

没有 `file_pages` 时，从 `plan.frame` 取得匿名/复制页并调用 `map_4k_concurrent`。
`AlreadyMapped` 表示竞争失败，返回 `Stale`；成功则清空 `plan.frame` 并增加
`mapped_pages` 计数。

### 8. COW 的旧 `populate` 回退

`CowBackend::populate` 在地址空间写锁内逐后端页执行：

- PTE 已存在且本次写、PTE 不可写：调用 `handle_cow_fault`。
- PTE 已存在且权限足够：计为已处理。
- PTE 不存在：调用 `alloc_new_at`。
- 其他页表错误：返回 `BadAddress`。

`handle_cow_fault` 先看该 VA 是否有直接映射的文件页 pin：

- 有 pin：分配新帧、复制缓存页、传统 `pt.remap`，成功后释放文件 pin。
- 没有 pin：查匿名引用数。引用数为 1 时只 `pt.protect` 升级权限；大于 1 才分配、复制
  和 remap，并减少旧 PTE 的引用。

`alloc_new_at` 分配清零页；若后端带文件，就在仍持有地址空间写锁时调用 `file.read_at`
填充有效区，随后传统 `pt.map`。这正是两阶段快路径试图避开的锁内慢 I/O，但非 4 KiB
页和未覆盖状态当前仍依赖它。

## 十一、并发 PTE 操作为什么安全

`vendor/page_table_multiarch/src/bits64.rs:213-263` 的两个接口是快路径能在 AddrSpace
读锁下提交的基础。

### 1. `map_4k_concurrent`

```rust
let slot = self.leaf_entry_atomic(vaddr, true)?;
let entry = PTE::new_page(target.align_down(PageSize::Size4K), flags, false).bits();
slot.compare_exchange(0, entry, Ordering::AcqRel, Ordering::Acquire)
    .map(|_| ())
    .map_err(|_| PagingError::AlreadyMapped)
```

- `leaf_entry_atomic(vaddr, true)` 找到叶 PTE 的 `AtomicUsize`；`true` 允许用 CAS 创建缺少
  的中间页表。
- `target.align_down(4K)` 确保 PA 页对齐。
- `PTE::new_page(...).bits()` 构造机器 PTE 位值。
- `compare_exchange(0, entry, ...)` 只允许把空 PTE 变成新 PTE。
- `AcqRel` 使此前页内容和页表构造在发布后对其他 CPU 可见；失败时 `Acquire` 观察胜者
  发布的状态。
- 一个线程成功，其他线程得到 `AlreadyMapped`，不会互相覆盖。

### 2. `remap_4k_concurrent_if`

循环读取当前 PTE：

- 不 present 返回 `NotMapped`。
- PA 已不是 `expected_paddr` 返回 `Ok(false)`。
- 否则构造新 PTE，对完整旧 bits 做 CAS。
- CAS 因同时修改失败就重读并重试。
- 成功后刷新该 VA 的 TLB 并返回真。

它比较的不只是“这个槽非空”，而是确认仍指向 prepare 固定的旧物理页。这正是无锁复制
后不覆盖并发变化的条件。

## 十二、folio 页缓存逐行读

### 1. 大小常量

`vendor/axfs/src/highlevel/file.rs:314-333`：

- `PAGE_SIZE = 4096`：基页是 4 KiB。
- `FOLIO_INDEX_SHARDS = 64`：页号索引分 64 把锁。
- `MAX_FOLIO_ORDER = 4`：最大阶数 4。
- `MAX_FOLIO_PAGES = 1 << 4 = 16`：数组为最大 16 个子页预留。
- 因而最大 folio 字节数是 `16 * 4096 = 65536`，即 64 KiB。
- `READAHEAD_WORKERS = 8`：最多启动 8 个异步预读
  worker。

### 2. `FolioState`

```rust
struct FolioState {
    loading: bool,
    uptodate: bool,
    error: Option<VfsError>,
    dirty: u16,
    writeback: u16,
    truncated: bool,
}
```

- `loading`：装载者是否尚未完成。
- `uptodate`：数据是否可供正常读取/映射。
- `error`：装载失败原因。
- `dirty`：16 位位图，每位对应一个 4 KiB 子页是否被修改。
- `writeback`：每个子页是否正在写回的位图。
- `truncated`：文件截断/失效已使该 folio 不可再发布。

`u16` 正好容纳最大 16 个子页；当前设计若把 `MAX_FOLIO_ORDER` 提高到 5，不能只改一个
常量，还必须扩大这些位图和 `page_locks`。

### 3. `Folio`

```rust
pub struct Folio {
    index: u32,
    order: u8,
    frame: FolioFrame,
    self_ref: Weak<Folio>,
    state: Mutex<FolioState>,
    page_locks: [Mutex<()>; MAX_FOLIO_PAGES],
    wait: WaitQueue,
    last_used: AtomicU64,
}
```

- `index`：folio 第一个文件基页号。
- `order`：基页数量的二进制阶数。
- `frame`：物理连续、自然对齐的页缓存内存。
- `self_ref`：指向自身 `Arc` 的弱引用，让借用的 `FolioPage` 能生成稳定强引用。
- `state`：短临界区保护状态位；不应拿它等待块 I/O。
- `page_locks`：每个 4 KiB 子页独立互斥，避免不同子页无谓串行。
- `wait`：等待 `loading` 或 `writeback` 状态变化时睡眠。
- `last_used`：页缓存 LRU 近似时钟值。

`new_loading(index, order)`：

- 两个 `debug_assert` 确认 order 不超上限、index 按 `2^order` 自然对齐。
- `alloc_folio_frame(order)` 分配连续页。
- `Arc::new_cyclic` 允许构造期间生成 `Weak<Folio>` 存到 `self_ref`。
- 初态是 `loading=true`、`uptodate=false`、无错误、无脏页、未截断。
- 每个子页锁独立初始化。
- `FILE_CACHE_CLOCK.fetch_add(1, Relaxed)` 分配近似最近使用序号；这里只要求唯一趋势，
  不用它同步数据。

辅助方法：

- `page_count = 1 << order`。
- `byte_len = page_count * PAGE_SIZE`。
- `contains(pn)` 检查 `index <= pn < index + page_count`。
- `page_offset = pn - index` 得子页数组下标。
- `paddr_at(page)` 用 folio 起始直接映射 VA 加字节偏移，再转 PA。
- `data_mut_unchecked` 构造覆盖整个 folio 的可写 slice；调用者必须保证没有冲突访问。

`complete_load(result)`：

1. 锁 state 并把 `loading=false`。
2. 成功且未 truncated：`uptodate=true`，清 error。
3. I/O 成功但并发 truncate：仍设为不可用和 `Io`。
4. I/O 失败：保存错误。
5. 显式 `drop(state)` 后 `notify_all`，避免唤醒者立刻又阻塞在状态锁上。

`wait_uptodate()`：

- `wait_until(|| !state.loading)` 睡到装载完成。
- 再锁 state；只有 `uptodate && !truncated` 返回成功，否则返回保存的错误或默认 `Io`。

### 4. `FolioPageRef` 和 `FolioPage`

`FolioPageRef { folio: Arc<Folio>, page: u8 }` 是可克隆的长期 pin：

- `Arc<Folio>` 固定整块 folio 内存。
- `page` 指定其中一个 4 KiB 子页。
- `paddr()` 计算这个子页 PA。

`FolioPage<'a>` 是持有子页锁的短期访问 guard：

- `folio: &'a Folio` 是借用。
- `_page_guard: MutexGuard<'a, ()>` 保证对象活着期间该子页互斥。
- `page_ref()` 通过 `self_ref.upgrade()` 生成长期 pin。
- `mark_dirty()` 在状态位图中设置本子页位。
- `data()` 只返回该子页的 4096 字节 slice，不暴露相邻子页。

### 5. 每 inode 的 `FileAddressSpace`

```rust
struct FileAddressSpace {
    folio_index: [RwLock<BTreeMap<u32, Arc<Folio>>>; 64],
    folio_heads: Mutex<BTreeMap<u32, Arc<Folio>>>,
    evict_listeners: Mutex<LinkedList<...>>,
    invalidate_lock: SleepRwLock,
    append_lock: SleepRwLock,
}
```

- 一份 `CachedFileRecord` 对应文件系统指针和 inode 号组成的 key，所以同一 inode 的
  `CachedFile` 共享同一个 `FileAddressSpace`。
- `folio_index[hash(pn)]` 是查找快路径；folio 的每个基页号都插入一项。
- `folio_heads` 只按 folio 首页保存一项，用于遍历、回收和发布多 shard 时串行化结构
  更新。
- `evict_listeners` 通知 mmap 后端撤掉仍指向将被回收页的 PTE。
- `invalidate_lock` 普通读取/缺页拿睡眠读锁，truncate 和破坏性失效拿写锁。
- `append_lock` 单独协调文件 append，不是缺页主锁。

这叫“per-inode address_space”，不是“每个进程一份”。同一文件被多个进程映射时，
它们能共享缓存 folio，但各自仍有自己的 PTE 和映射 pin。

### 6. `lookup_folio`

```rust
let folio = self.shared().folio_index[Self::folio_shard(pn)]
    .read()
    .get(&pn)
    .cloned();
if let Some(folio) = folio.as_ref() {
    folio.touch();
}
folio
```

- `folio_shard(pn) = pn % 64` 选择锁。
- 读锁只保护 BTreeMap 查找。
- `.cloned()` 增加 `Arc` 引用后即可释放索引锁。
- 命中后更新 `last_used`。
- 最后一行返回 `Option<Arc<Folio>>`。

### 7. `publish_folio`

```rust
let mut heads = self.shared().folio_heads.lock();
let end = folio.index + folio.page_count() as u32;
for pn in folio.index..end {
    if folio_index[shard(pn)].read().contains_key(&pn) {
        return false;
    }
}
heads.insert(folio.index, folio.clone());
for pn in folio.index..end {
    folio_index[shard(pn)].write().insert(pn, folio.clone());
}
true
```

- `folio_heads` 锁把“检查所有 shard + 发布所有 slot”组成一个创建者临界区。
- 第一轮确认范围内没有已有 folio，防止重叠 folio。
- 先把规范 owner 放入 `heads`，再为每个基页号发布索引。
- 成功返回真；若冲突返回假，调用者丢弃候选并观察胜者。
- 这把锁只在 cache miss 的发布/删除路径使用，普通 cache hit 不经过它。

### 8. `get_or_publish_folio`

```rust
if let Some(folio) = self.lookup_folio(pn) {
    return Ok((folio, false));
}
let mut order = preferred_order.min(MAX_FOLIO_ORDER);
loop {
    let pages = 1u32 << order;
    let index = pn & !(pages - 1);
    let folio = match Folio::new_loading(index, order) { ... };
    if self.publish_folio(&folio) {
        return Ok((folio, true));
    }
    if let Some(folio) = self.lookup_folio(pn) {
        return Ok((folio, false));
    }
    if order > 0 { order -= 1; }
}
```

- 返回元组第二项 `loader`：`true` 表示调用者赢得发布竞争，负责 I/O；`false` 表示它
  必须等待已有 folio。
- `preferred_order.min(4)` 限制最大 64 KiB。
- `pn & !(pages - 1)` 清除低位，把页号向下自然对齐到 folio 边界。
- 大 folio 分配 `NoMemory` 时逐阶降级，不因连续大块不足直接让缺页失败。
- `publish_folio` 失败后再 lookup 关闭“第一次 lookup 与另一个线程发布之间”的竞态。
- order 已为 0 且一时仍找不到时继续重试；不会构造重叠对象。

### 9. `load_folio`

```rust
let data = unsafe { folio.data_mut_unchecked() };
let result = if zeroed || self.in_memory() {
    data.fill(0);
    Ok(())
} else {
    self.read_fully_at(file, data, folio.index * PAGE_SIZE, file_len)
        .map(|read| data[read..].fill(0))
};
folio.complete_load(result);
folio.wait_uptodate()
```

- loader 独占尚未发布为 uptodate 的数据区，所以使用 unchecked 全 folio slice。
- `zeroed` 或 tmpfs 新页直接清零。
- 磁盘文件从 folio 起始文件偏移读取，`read_fully_at` 尽量填完整块。
- 实际读取不足 folio 长度时把尾部清零，覆盖 EOF 后字节。
- 不论成功失败都调用 `complete_load` 改状态并唤醒等待者。
- 最后自己也通过 `wait_uptodate` 统一检查 truncate/错误后的最终状态。

### 10. `with_page_or_insert_impl`

这是 demand cache lookup 的核心：

1. profile feature 打开时建立 `PageCache` 范围；关闭时这些行不编译。
2. `may_retry_failed_folio = true` 允许前台 demand 发现失败的推测预读 folio 后删除并重试
   一次。
3. 第一段拿 invalidate 读锁查 resident folio：
   - 成功就 `wait_uptodate`。
   - 有效则 `with_folio_page` 锁故障子页并运行回调。
   - 第一次发现失败就移除并进入重试；第二次失败返回错误。
4. miss 后先按内存水位尝试回收。
5. 再拿 invalidate 读锁，取得文件、长度并调用 `get_or_publish_folio`。
6. `loader=true` 时自己 `load_folio`；否则等待胜者 `wait_uptodate`。
7. 装载失败就从索引移除；在重试预算内 `continue`，否则返回错误。
8. 成功后用 `with_folio_page` 只锁目标子页并调用 `f`。

这里 `loop` 的意义不是无限吞掉 I/O 错误。`may_retry_failed_folio` 保证失败 folio最多按
demand I/O 重试一次；正常循环主要处理发布竞争。

### 11. `fault_page_ref`

```rust
let file_pages = self.len()?.div_ceil(PAGE_SIZE as u64).max(1);
let order = (u64::BITS - 1 - file_pages.leading_zeros()) as u8;
self.with_page_or_insert_impl(
    pn,
    false,
    order.min(MAX_FOLIO_ORDER),
    |page, _| Ok(page.page_ref()),
)
```

- 文件字节长度向上除 4 KiB 得总页数，最小取 1。
- `BITS - 1 - leading_zeros` 计算 `floor(log2(file_pages))`。
- 再封顶 order 4。文件至少 16 页时通常请求 64 KiB folio；小文件请求不超过自身量级
  的自然对齐 folio。
- `zeroed=false` 表示从文件装载。
- 回调把短期 `FolioPage` 变成长期 `FolioPageRef`。

### 12. cache pin 提交前的最终验证

`with_valid_page_ref`：

1. 拿 `invalidate_lock.read()`，排除 truncate/invalidate 写侧。
2. 再 lookup 当前 `pn`。
3. `Arc::ptr_eq` 检查索引中的 folio 正是计划持有的 folio。
4. 检查子页 offset 也一致。
5. 锁 state 并拒绝 `loading`、非 `uptodate` 或 `truncated`。
6. 写映射且非 tmpfs 时设置对应 dirty 位。
7. 释放 state，执行短 commit 闭包。

因此 File/COW 后端不会仅凭一个旧 `FolioPageRef` 在 truncate 后继续发布新 PTE。

### 13. 异步 `readahead` 与 demand fault 的区别

`CachedFile::readahead(offset, count)`：

- 空范围或 tmpfs 直接成功。
- 截到 EOF 和 `MAX_READAHEAD_BYTES`（1 MiB）。
- 遍历范围，已有 folio（包括 loading）就跳过，绝不等待。
- 对 miss 创建自然阶 folio并收集自己成为 loader 的项。
- 释放 invalidate 读锁后，把 job 放入全局队列。
- 最多唤醒 8 个 worker；worker 调 `load_readahead_folio`。

所以：

- demand `fault_page_ref`：故障线程负责或等待装载，必须拿到页才能返回用户态。
- advisory `readahead`：调用者只发布任务，不等待已有/新建 folio完成。

不能把二者混为一谈。

## 十三、TLB 与可执行页同步

`kernel/src/mm/aspace/mod.rs:133-149`：

```rust
#[inline]
fn synchronize_executable_fault(access_flags: PageFaultFlags) {
    if access_flags.contains(MappingFlags::EXECUTE) {
        #[cfg(target_arch = "riscv64")]
        riscv::asm::fence_i();
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            core::arch::asm!("ibar 0", options(nostack));
        }
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    let _ = access_flags;
}
```

- `#[inline]` 建议编译器把短函数内联，不保证一定内联。
- 只有本次是执行缺页才需要取指同步。
- `#[cfg]` 是编译期选择，不是运行时 if；每个目标只包含自己的指令。
- RISC-V `fence.i` 让当前 hart 后续取指看到此前数据写入形成的新指令。
- LoongArch 的 I/D cache 由硬件保持一致；`ibar 0` 消除剩余取指流水线/屏障风险。这里
  用 inline asm，所以是 `unsafe`。
- `options(nostack)` 告诉编译器汇编不修改栈。
- 最后的 cfg 分支只为其他架构“使用”参数，避免 unused warning。

TLB 刷新解决“虚拟地址翻译缓存”，`fence.i/ibar` 解决“指令获取状态”；二者不是同一
件事，不能互相替代。

## 十四、用三个具体例子串起代码

### 例 1：两个线程同时读取同一私有可执行文件页

1. 两个 CPU 都 prepare 出 Cow file plan。
2. A 先发布 `loading` 64 KiB folio并读取；B lookup 到同一 folio并睡眠。
3. A 完成后唤醒 B；两者都持有目标 `FolioPageRef`。
4. A 在 PTE CAS 中获胜并插入 pin。
5. B commit 看到 PTE 已满足权限，返回 `Handled`；其多余计划引用自动释放。
6. 两边刷新本地 TLB；执行缺页还各自在自己的 CPU 执行取指屏障。

发生了一次 folio I/O 和一次 PTE 发布，不是两份重复读取。

### 例 2：fork 后两个 CPU 写不同匿名页

1. 两页 PTE 都是只读共享 COW。
2. 每个 fault prepare 在各自 frame ref shard 增加临时源引用。
3. 两个 CPU 不拿 AddrSpace 锁并行分配和复制不同物理页。
4. commit 阶段都拿地址空间读锁；不同叶 PTE 的 CAS 可并行。
5. 各自替换自己的 PTE，减少原 PTE 引用；计划析构再减少临时引用。

这里避免的是“不同页复制被进程级写锁串行”。同一页的两个写者仍只能有一个 CAS 胜者。

### 例 3：execute 期间另一个线程 `munmap`

1. fault prepare 得到计划后释放读锁。
2. `munmap` 获得写锁并删除 VMA/PTE。
3. fault execute 可能已经分配或读出页。
4. commit 重新拿读锁后 `areas.find(vaddr)` 失败，直接返回 false。
5. `CowFaultPlan::Drop` 释放未提交 frame；`FileFaultPlan` 的 `FolioPageRef` 正常析构。

旧计划不会复活已经 unmap 的映射。这就是重新验证不可省略的原因。

## 十五、阅读结论

当前缺页机制的核心不是“用了 folio”这一个名词，而是几项配合：

```text
VMA 读锁短检查
  + 锁外分配/复制/I/O
  + per-inode 可变阶 folio 合并相同读取
  + FolioPageRef 固定被 PTE 使用的缓存页
  + commit 前重新验证
  + 叶 PTE CAS
  + 失败计划自动释放
```

它已经把常见的 4 KiB File/COW 慢操作移出进程地址空间独占锁，并允许不同页的准备和
提交并行；它没有消灭地址空间锁，也没有把 ext4、同一 folio 首次 I/O、truncate、
同一 PTE 竞争或所有 fallback 变成无锁。评价实际优化效果时，应分别测量：

- File/COW 快路径命中比例与 fallback 比例；
- 同 folio 等待和重复 I/O 是否下降；
- AddrSpace 写锁等待是否下降；
- ext4 和 VirtIO 的剩余 off-CPU 等待；
- 固定时间内完成的 crate 数，而不只看 CPU busy 百分比。

这才与当前代码能够提供的保证相符。
