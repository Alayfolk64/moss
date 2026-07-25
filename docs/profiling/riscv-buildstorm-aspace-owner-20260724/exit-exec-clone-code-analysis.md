# AddrSpace 三大写锁热点源码逐行分析

本文只分析 2026-07-24 当前分支 `smp-scheduler-optimization`、提交
`7ad5cc6b` 上与下面三项数据直接相关的代码：

| 持有者标签 | 测得累计持锁时间 | 表中次数 | 表中平均值 | 最大实测样本 |
|---|---:|---:|---:|---:|
| `exit clear` | **306.57 s** | 393,719 | 0.779 ms | 48.75 ms |
| `exec clear` | **252.87 s** | 708,143 | 0.357 ms | 7.48 ms |
| `clone` | **247.24 s** | 1,552 | **159.31 ms** | **445.26 ms** |

全文先讲总体流程，再逐行解释。逐行部分对每个有业务含义的标识符都拆词说明；
`let`、`mut`、`&`、`::`、`?` 等重复出现的 Rust 语法统一在“语法词典”中解释，
避免每一行重复同一句话。

## 一、先正确理解这三行数据

### 1. `累计持锁时间`是什么

探针在写锁**成功取得之后**开始计时，在写锁 guard 被销毁、锁被释放之后结束
计时。因此它测量的是 hold time，也就是持有时间，不包含等待取得写锁的时间。

普通 AddrSpace 持有者使用 1/16 抽样。每抽中一次，记录的持续时间乘 16，
用于估算总体累计值。`exit clear` 和 `exec clear` 在这次测量中不抽样，
因为它们原本被拆成许多小批次，需要避免长批次恰好没有被抽中。

累计值是 task-time：

```text
累计持锁时间 = 每个被记录持锁区间的持续时间之和（抽样事件再乘抽样倍率）
```

它不是墙钟时间。两个 CPU 可以同时持有两个**不同 AddrSpace 实例**的写锁，
这两个区间都会被相加。

### 2. 393,719 和 708,143 不是进程数量

这次完整测量对应提交 `965bd66e`。当时退出和 exec 回收每次只处理：

```text
256 × 4 KiB = 1 MiB 虚拟地址范围
```

处理一批就释放写锁、把请求重新放回队列。因此：

- 393,719 是 `exit clear` 写锁批次数；
- 708,143 是 `exec clear` 写锁批次数；
- 0.779 ms 和 0.357 ms 是每个**批次**的平均持锁时间；
- 它们都不是一次完整退出或一次完整 exec 的平均值。

提交 `7ad5cc6b` 已删除 `clear_batch`。当前代码每个死亡地址空间只从队列取出
一次、取得一次写锁、执行一次完整 `clear()`。所以上表能说明“清理总工作量
很大”和“旧分批策略批次数异常”，但不能预测当前版本新的事件次数与平均值。

### 3. `clone` 的 1,552 也不是 clone 系统调用数量

预构建实际抽中 97 个 `Clone` 写锁持有区间：

```text
97 × 16 = 1,552 个估算写锁区间
```

一次不带 `CLONE_VM` 的 fork/clone 至少可能产生：

1. 父 AddrSpace 写锁区间；
2. `AddrSpace::try_clone` 内部的子 AddrSpace 写锁区间；
3. `copy_from_kernel` 前再次取得的子 AddrSpace 写锁区间。

这些区间可能嵌套。父锁和子锁同时被一个任务持有时，两段时间都会进入累计值。
所以：

- 159.31 ms 是“每个被标记为 `Clone` 的写锁区间”的估算平均值；
- 445.26 ms 是一个实际抽中的最长写锁区间；
- 247.24 s 不能直接除以 fork 次数；
- 247.24 s 也不能直接当作 clone 独占的墙钟损失。

尽管如此，单个区间达到数百毫秒，仍然足以证明 clone 在活地址空间上形成了
真正的长写锁临界区。

## 二、必须先认识的对象

### 1. `ProcessData.aspace`

当前进程通过下面的字段持有地址空间：

```rust
aspace: RwLock<Arc<AddrSpaceLock>>,
```

这里有两层不同的同步：

| 层 | 类型 | 保护什么 |
|---|---|---|
| 外层 | `RwLock<Arc<AddrSpaceLock>>` | 保护“当前进程指向哪个地址空间”这个指针 |
| 内层 | `AddrSpaceLock` | 保护被指向的 AddrSpace 内容 |

`execve` 的 `replace_aspace` 修改的是外层指针；缺页、mmap、clone、clear 等访问
的是内层 AddrSpace。不能把两把锁混成一把。

当前两个访问方法是：

```rust
pub fn aspace(&self) -> Arc<AddrSpaceLock> {
    self.aspace.read().clone()
}

pub fn replace_aspace(&self, new_aspace: Arc<AddrSpaceLock>) -> Arc<AddrSpaceLock> {
    core::mem::replace(&mut *self.aspace.write(), new_aspace)
}
```

逐行解释：

- `pub fn aspace(&self) -> Arc<AddrSpaceLock> {`
  - getter 名直接叫 `aspace`；
  - `&self` 不修改 ProcessData；
  - 返回一份拥有强引用的 Arc，而不是借用内部字段。
- `self.aspace.read().clone()`
  - `self.aspace`：外层指针锁；
  - `read()`：只读取得当前 Arc；
  - `clone()`：Arc 强引用数加一；
  - 外层读锁在语句结束时释放，调用者以后独立持有 Arc。
- `pub fn replace_aspace(&self, new_aspace: Arc<AddrSpaceLock>) -> Arc<AddrSpaceLock> {`
  - `replace`：替换；
  - `new_aspace` 按值传入，调用者把 Arc 所有权交给函数；
  - 返回值是旧 Arc。
- `core::mem::replace(&mut *self.aspace.write(), new_aspace)`
  - `self.aspace.write()`：取得外层写锁；
  - `*`：把 write guard 解引用成内部 Arc；
  - `&mut`：取得该 Arc 槽位的可变引用；
  - `core::mem::replace`：把 new_aspace 写入槽位，同时把旧值移动出来；
  - 操作只交换 Arc，不遍历 VMA、不复制页表；
  - write guard 在表达式结束时释放。

名称拆解：

- `Process`：进程；
- `Data`：与进程关联的数据；
- `aspace`：`address space` 的缩写，即地址空间；
- `RwLock`：`read-write lock`，读写锁；
- `Arc`：`atomically reference-counted`，原子引用计数智能指针；
- `AddrSpaceLock`：包住地址空间的锁对象。

### 2. `AddrSpaceLock` 和 `AddrSpace`

当前定义是：

```rust
pub struct AddrSpaceLock(BlockingRwLock<AddrSpace>);
```

逐词解释：

- `pub`：该类型对其他模块公开；
- `struct`：定义结构体；
- `AddrSpaceLock`：`Address Space Lock`，地址空间锁；
- `BlockingRwLock`：竞争失败时任务睡眠，而不是始终自旋的读写锁；
- `<AddrSpace>`：这把锁内部保存的值是 `AddrSpace`；
- 圆括号形式：这是只有一个匿名字段的 tuple struct。

`AddrSpace` 内部最重要的是：

```text
AddrSpace
├── va_range：整个用户虚拟地址范围
├── areas：MemorySet，按起始地址组织的 VMA 集合
└── pt：PageTable，真正的页表
```

名称拆解：

- `Addr`：address，地址；
- `Space`：空间；
- `va`：virtual address，虚拟地址；
- `range`：范围；
- `areas`：多个 memory areas，也就是多个 VMA；
- `pt`：page table，页表；
- `VMA`：virtual memory area，一段连续且属性一致的虚拟内存区域；
- `PTE`：page table entry，页表项。

### 3. 为什么是写锁

`clear` 会删除 VMA、删除 PTE 并更新 backend 引用；`clone` 会遍历父 VMA，
还会把父进程原本可写的 PTE 改成只读 COW PTE。这些操作会改变结构，不能和
同一地址空间上的 mmap/munmap/mprotect 或缺页提交任意交错，所以当前实现用
AddrSpace 写锁。

但不同场景的含义不同：

- `exit clear`/`exec clear` 操作的是已经死亡或已被替换的旧地址空间；
- `clone` 操作的父地址空间仍在运行，其他父线程仍可能缺页或访问 futex。

因此长 `clone` 写锁会直接阻塞活 reader；死亡地址空间上的长 clear 锁通常
不阻塞正常用户 reader，但仍消耗 CPU，并竞争物理页引用、页表和 backend 锁。

## 三、三条热点的总体流程

### 1. `exit clear`

```text
最后一个线程进入退出路径
    |
    +-- process.exit_thread(...) 判断 last_thread
    |
    +-- 统计 RSS、释放 AIO/cap/time/mmap/fanotify/file 等资源
    |
    +-- reclaim_process_address_space(...)
            |
            +-- 取得 ProcessData 中的 Arc<AddrSpaceLock>
            |
            +-- 检查是否还有其他活进程共享同一个 Arc
            |
            +-- 没有共享者：以 ExitClear 标签放入全局回收队列
                    |
                    +-- 确保只有一个后台回收 worker
                    |
                    +-- worker 取出请求
                    |
                    +-- 取得旧 AddrSpace 写锁并开始 ExitClear 探针
                    |
                    +-- AddrSpace::clear
                            |
                            +-- MemorySet::clear
                            +-- 逐 VMA 调用 backend.unmap
                            +-- 逐驻留页删除 PTE
                            +-- 释放 COW frame/file-page 引用
                            +-- 清空 VMA BTreeMap
```

为什么它耗时：

1. 构建过程创建和退出大量 rustc、链接器、build script 等进程；
2. 每个进程可能有很多 VMA；
3. COW 和文件映射的 unmap 会按页遍历；
4. 每个驻留 COW 页还要更新 frame 引用计数；
5. 测量版本又把每个 VMA 按 1 MiB 反复切分，制造大量额外工作。

当前代码已经删除第 5 项，但前 1～4 项仍然存在。

### 2. `exec clear`

```text
execve 已解析并验证新程序
    |
    +-- 在旁边创建 new_aspace
    +-- 装载 ELF、用户栈和参数
    +-- 保存新页表根 page_table_root
    |
    +-- ProcessData.replace_aspace(new_aspace)
            |
            +-- 原子地替换进程的地址空间指针
            +-- 返回 old_aspace
    |
    +-- 检查 old_aspace 是否仍被其他活进程共享
    |
    +-- 切换当前任务到新 page_table_root
    |
    +-- 不共享：以 ExecClear 标签异步回收 old_aspace
    +-- 仍共享：只 drop 当前 Arc，不能清掉别人的映射
```

先构造新地址空间、成功后再替换的好处是：如果 ELF 装载失败，旧程序仍可继续，
不会得到一半被拆掉的地址空间。

`vfork` 子进程可能暂时和父进程共享地址空间。`old_aspace_shared` 检查用于避免
子进程 exec 时误清父进程的地址空间。

### 3. `clone`

```text
sys_clone / clone3 / fork / vfork
    |
    +-- flags 含 CLONE_THREAD：
    |       共享整个 ProcessData，不复制地址空间
    |
    +-- 新进程且含 CLONE_VM：
    |       共享 Arc<AddrSpaceLock>，不复制地址空间
    |
    +-- 新进程且不含 CLONE_VM：
            |
            +-- 取得父 AddrSpace 写锁（Clone）
            |
            +-- AddrSpace::try_clone
            |       |
            |       +-- 创建空的子 AddrSpace
            |       +-- 取得子 AddrSpace 写锁（Clone）
            |       +-- 遍历父进程每一个 VMA
            |       +-- 按 backend 执行 clone_map
            |       +-- COW 映射：遍历虚拟页
            |       |       +-- 查询父 PTE
            |       |       +-- 父 PTE 去掉 WRITE
            |       |       +-- 子 PTE 指向同一物理页
            |       |       +-- 增加共享页引用
            |       +-- 在子 MemorySet 安装新 VMA
            |
            +-- 再次取得子 AddrSpace 写锁（Clone）
            +-- copy_from_kernel
            +-- 处理 MADV_WIPEONFORK 范围
            +-- 把子页表根写进新任务上下文
```

这里真正必须串行的是最终改变父 PTE 为 COW，以及验证父 VMA 没有变化。
当前实现把以下慢工作也放在父写锁覆盖期内：

- 创建并初始化子地址空间；
- 遍历所有父 VMA；
- 对存在驻留页的 COW VMA 遍历整个虚拟范围；
- 分配子页表节点；
- 建立所有子 PTE；
- 可能还覆盖了后续对子地址空间的补充工作。

这就是 `clone` 单次写锁达到数百毫秒的直接源码原因。

## 四、Rust 语法词典

后文逐行解释使用以下统一含义：

| 记号/单词 | 含义 |
|---|---|
| `pub` | 对外可见 |
| `pub(crate)` | 只对当前 crate 可见；crate 可理解为当前 Rust 包 |
| `fn` | function，定义函数 |
| `struct` | 定义结构体 |
| `enum` | 定义枚举，值只能是所列变体之一 |
| `impl` | 为类型实现方法或 trait |
| `let` | 新建局部绑定 |
| `mut` | mutable，允许修改 |
| `self` | 当前对象；`Self` 是当前类型 |
| `&T` | 共享借用，只读引用 |
| `&mut T` | 独占可变借用 |
| `Arc<T>` | 多拥有者、线程安全的引用计数指针 |
| `Option<T>` | 可能是 `Some(T)`，也可能是 `None` |
| `Result<T, E>` | 可能成功得到 `T`，也可能失败得到 `E` |
| `AxResult<T>` | 项目定义的 `Result<T, AxError>` |
| `::` | 访问模块、类型或枚举的成员 |
| `.` | 调用对象方法或访问字段 |
| `?` | 遇到错误立即从当前函数返回；成功时取出值 |
| `!condition` | 逻辑取反 |
| `&&` | 逻辑与，左边为假时不计算右边 |
| `||` | 在条件里是逻辑或；`|| {}` 也可表示无参数闭包 |
| `=>` | match 分支“匹配到左侧就执行右侧” |
| `|x|` | 闭包参数列表 |
| `{ ... }` | 作用域或代码块；最后一个无分号表达式是块返回值 |
| `;` | 结束语句，并丢弃表达式值 |
| `_` | 明确忽略一个值或字段 |
| `as` | 类型转换 |
| `<T>` | 泛型参数 |
| `<'a>` | 生命周期参数；表示引用至少在 `'a` 期间有效 |
| `()` | unit，无有用返回数据，也可表示空参数列表 |
| `drop(x)` | 显式销毁 `x`，触发其 `Drop` |
| `snake_case` | Rust 变量/函数命名法，例如 `old_aspace` |
| `CamelCase` | Rust 类型/枚举变体命名法，例如 `AddrSpace` |

## 五、探针为什么恰好测到“持锁时间”

### 1. 写锁 guard 的结构

源码：`kernel/src/mm/aspace/mod.rs:283-291`

```rust
pub(crate) struct ProfiledAddrSpaceWriteGuard<'a> {
    guard: BlockingRwLockWriteGuard<'a, AddrSpace>,
    #[cfg(all(feature = "guest-profile", target_arch = "riscv64"))]
    _profile: axsync::ProfileScope,
}
```

逐行解释：

- `pub(crate) struct ProfiledAddrSpaceWriteGuard<'a> {`
  - `Profiled`：带 profiling 的；
  - `AddrSpace`：地址空间；
  - `WriteGuard`：写锁 guard；guard 活着就代表锁仍被持有；
  - `'a`：guard 不能活得比原锁更久；
  - `{`：开始列出字段。
- `guard: BlockingRwLockWriteGuard<'a, AddrSpace>,`
  - `guard`：真正持有写锁的字段；
  - `Blocking`：竞争时允许阻塞；
  - `RwLock`：读写锁；
  - `WriteGuard`：写侧所有权凭证；
  - `<..., AddrSpace>`：guard 允许可变访问 AddrSpace。
- `#[cfg(all(feature = "guest-profile", target_arch = "riscv64"))]`
  - `cfg`：conditional compilation，条件编译；
  - `all`：所有条件同时成立；
  - `feature = "guest-profile"`：启用了 guest profiling；
  - `target_arch = "riscv64"`：目标架构是 RISC-V 64；
  - 不满足条件时，下一字段根本不进入二进制。
- `_profile: axsync::ProfileScope,`
  - `_profile`：profiling 区间对象；前导 `_` 表示它主要依赖析构副作用；
  - `axsync`：提供同步与探针接口的 crate；
  - `ProfileScope`：创建时开始计时，销毁时结束计时。
- `}`：结构体字段结束。

字段声明顺序是 `guard` 在前、`_profile` 在后。Rust 销毁结构体字段时按声明
顺序销毁，因此先释放真实写锁，再结束 profile interval。测到的结束点只比
实际 unlock 晚极短的析构调用开销。

### 2. `lock_profiled` 逐行

源码：`kernel/src/mm/aspace/mod.rs:312-325`

```rust
/// Takes the write lock and records how long `owner` keeps it.
pub(crate) fn lock_profiled(
    &self,
    owner: AddrSpaceWriteOwner,
) -> ProfiledAddrSpaceWriteGuard<'_> {
    #[cfg(not(all(feature = "guest-profile", target_arch = "riscv64")))]
    let _ = owner;
    let guard = self.0.write();
    ProfiledAddrSpaceWriteGuard {
        guard,
        #[cfg(all(feature = "guest-profile", target_arch = "riscv64"))]
        _profile: axsync::ProfileScope::new(owner.profile_event(), self as *const _ as usize),
    }
}
```

逐行解释：

- 注释 `Takes the write lock...`
  - `Takes`：取得；
  - `write lock`：写锁；
  - `records`：记录；
  - `how long`：多长时间；
  - `owner keeps it`：指定 owner 持有它多久。
- `pub(crate) fn lock_profiled(`
  - `lock`：取得锁；
  - `profiled`：这个取得动作会配套持锁探针。
- `&self,`
  - 共享借用锁对象；锁内部用原子状态实现内部可变性，不要求 `&mut self`。
- `owner: AddrSpaceWriteOwner,`
  - `owner`：调用者声明“这次写锁是谁持有”；
  - 类型是有限枚举，避免用任意字符串。
- `) -> ProfiledAddrSpaceWriteGuard<'_> {`
  - `->`：返回类型；
  - `'_`：编译器推导生命周期，它与 `&self` 绑定。
- `#[cfg(not(all(...)))]`
  - `not`：条件取反；
  - 表示在非 RISC-V profiling 构建中编译下一行。
- `let _ = owner;`
  - 非 profiling 构建不使用 `owner`；
  - 绑定给 `_` 防止 unused-variable 警告；
  - 不产生运行时探针。
- `let guard = self.0.write();`
  - `self.0`：tuple struct 的第 0 个字段，即真实 `BlockingRwLock`；
  - `write()`：阻塞直到取得写锁；
  - 这一行返回后才开始构造 profile，所以等待锁的时间不计入 hold time。
- `ProfiledAddrSpaceWriteGuard {`
  - 构造包装 guard，最后一个表达式没有分号，因此也是函数返回值。
- `guard,`
  - 字段简写，等价于 `guard: guard`。
- 第二个 `cfg`：
  - 只在 RISC-V guest-profile 中加入 `_profile` 字段。
- `_profile: axsync::ProfileScope::new(...)`
  - `new`：开始一个 RAII 计时区间。
- `owner.profile_event()`
  - 把 `ExitClear`、`ExecClear`、`Clone` 等 owner 转成 profiler 事件编号。
- `self as *const _ as usize`
  - 第一次 `as`：把当前锁对象引用转成常量裸指针；
  - `*const _`：只读裸指针，具体指向类型让编译器推导；
  - 第二次 `as`：把指针地址转成整数；
  - `object` 因而可区分不同 AddrSpaceLock 实例。
- 最后三个 `}`：
  - 依次结束结构体字面量、函数体。

### 3. 三个 owner 名字

```rust
ExitClear,
ExecClear,
Clone,
```

- `ExitClear`
  - `Exit`：进程退出；
  - `Clear`：清除旧地址空间的 VMA、PTE 和 backend 引用。
- `ExecClear`
  - `Exec`：`execve` 用新程序映像替换当前程序；
  - `Clear`：清除被替换的旧地址空间。
- `Clone`
  - clone/fork 创建新进程地址空间；
  - 这里既标记父 AddrSpace 写锁，也标记子 AddrSpace 写锁。

owner 到 profiler 事件的当前映射是：

```rust
Self::ExitClear => ProfileEvent::AddrSpaceExitClearHold,
Self::ExecClear => ProfileEvent::AddrSpaceExecClearHold,
Self::Clone => ProfileEvent::AddrSpaceCloneHold,
```

- `Self::ExitClear`
  - `Self` 是 `AddrSpaceWriteOwner`；
  - `ExitClear` 是调用者传入的枚举变体。
- `ProfileEvent::AddrSpaceExitClearHold`
  - `ProfileEvent` 是 profiler 能记录的事件枚举；
  - `AddrSpace` 指明子系统；
  - `ExitClear` 指明来源；
  - `Hold` 明确表示持有时间，不是 acquire wait。
- 第二行完全同理，只把 exit 换成 exec。
- 第三行把所有标为 `Clone` 的父/子写锁区间归入同一个
  `AddrSpaceCloneHold` 事件，因此报表不能只凭事件名区分父锁和子锁。

## 六、`exit clear` 当前代码逐行解释

### 1. 只在最后一个线程退出时回收

源码：`kernel/src/task/ops.rs:279-290`

```rust
let aspace = thr.proc_data.aspace();
let mmap_locked_bytes = if address_space_shared_with_live_process(&aspace, process.pid()) {
    0
} else {
    aspace.read().locked_bytes()
};
crate::syscall::release_process_mmap_resources(process.pid(), mmap_locked_bytes);
crate::syscall::release_process_fanotify_resources(process.pid());
crate::syscall::close_all_files_for_current_process();
reclaim_process_address_space(&thr.proc_data);
thr.proc_data.complete_vfork();
process.exit();
```

这段代码位于 `if last_thread { ... }` 内。也就是说普通线程退出不会清空整个
进程地址空间，只有 `process.exit_thread(...)` 判断这是进程最后一个线程时
才执行。

逐行解释：

- `let aspace = thr.proc_data.aspace();`
  - `thr`：thread，当前用户线程对象；
  - `proc_data`：该线程所属进程的共享 `ProcessData`；
  - `aspace()`：读取外层指针锁并 clone 一份 `Arc<AddrSpaceLock>`；
  - `aspace`：当前函数持有的地址空间强引用；
  - clone Arc 只增加引用计数，不复制页表或 VMA。
- `let mmap_locked_bytes = if address_space_shared_with_live_process(...`
  - `mmap`：memory map；
  - `locked`：被 `mlock` 锁定、不可被常规回收的；
  - `bytes`：字节数；
  - `address_space_shared_with_live_process`：检查同一 AddrSpace 是否仍由其他
    活进程使用；
  - `&aspace`：借用 Arc 本身，不转移所有权；
  - `process.pid()`：当前退出进程的 process ID，用于排除自己。
- `0`
  - 如果还有别的活进程共享该地址空间，当前进程退出不能把共享地址空间的
    locked bytes 全部扣除，所以这里记 0。
- `} else {`
  - 没有其他活共享者时进入此分支。
- `aspace.read().locked_bytes()`
  - `read()`：取得 AddrSpace 读锁；
  - `locked_bytes()`：读取此地址空间累计的 mlock 字节数；
  - 只读取统计，不改变映射。
- `};`
  - 结束 if 表达式，并把结果绑定到 `mmap_locked_bytes`。
- `release_process_mmap_resources(process.pid(), mmap_locked_bytes);`
  - `release`：释放；
  - `process`：进程级；
  - `mmap_resources`：mmap/mlock 相关的外围记账；
  - 这还不是 VMA/PTE 的实际 clear。
- `release_process_fanotify_resources(process.pid());`
  - 释放该进程的 fanotify 资源；
  - 与地址空间 clear 没有直接关系，但必须在进程退出时完成。
- `close_all_files_for_current_process();`
  - 关闭当前进程的文件描述符；
  - 关闭文件可能触发文件系统清理，但不属于 `ExitClear` 探针区间。
- `reclaim_process_address_space(&thr.proc_data);`
  - `reclaim`：回收；
  - `process_address_space`：进程地址空间；
  - 只把回收请求送到异步队列，不在这一行同步遍历 PTE。
- `thr.proc_data.complete_vfork();`
  - 如果当前进程是 vfork 子进程，唤醒等待它退出或 exec 的父进程；
  - `complete`：完成；
  - `vfork`：父子暂时共享地址空间的 fork 变体。
- `process.exit();`
  - 把进程状态转为退出；
  - 地址空间 Arc 仍可能由 `ProcessData`、任务表或回收队列持有，不等于此刻
    立即析构 AddrSpace。

### 2. 判断地址空间是否仍被其他活进程共享

源码：`kernel/src/task/ops.rs:101-110`

```rust
pub(crate) fn address_space_shared_with_live_process(
    aspace: &Arc<crate::mm::AddrSpaceLock>,
    owner_pid: Pid,
) -> bool {
    processes().iter().any(|other| {
        other.proc.pid() != owner_pid
            && !other.proc.is_zombie()
            && Arc::ptr_eq(&other.aspace(), aspace)
    })
}
```

逐行解释：

- `pub(crate) fn address_space_shared_with_live_process(`
  - `address_space`：地址空间；
  - `shared`：共享；
  - `with`：与；
  - `live_process`：仍存活的进程；
  - 整个名字直接表达“是否与其他活进程共享地址空间”。
- `aspace: &Arc<crate::mm::AddrSpaceLock>,`
  - `aspace`：待检查的目标 Arc；
  - `crate::mm`：当前内核 crate 的 memory-management 模块；
  - 传引用是因为这里只比较，不消费 Arc。
- `owner_pid: Pid,`
  - `owner`：发起检查的进程；
  - `pid`：process identifier；
  - 用它排除当前进程自己。
- `) -> bool {`
  - 返回布尔值：共享为 true，不共享为 false。
- `processes().iter().any(|other| {`
  - `processes()`：收集当前进程表中的 `Vec<Arc<ProcessData>>`；
  - `iter()`：遍历；
  - `any`：只要任一元素满足条件就返回 true，并短路停止；
  - `other`：当前检查的候选“其他进程”。
- `other.proc.pid() != owner_pid`
  - `!=`：不相等；
  - 排除拥有该地址空间、正在退出或 exec 的当前进程。
- `&& !other.proc.is_zombie()`
  - `is_zombie`：是否已是僵尸进程；
  - `!`：要求它不是 zombie，也就是仍可能实际使用地址空间。
- `&& Arc::ptr_eq(&other.aspace(), aspace)`
  - `ptr_eq`：pointer equality，比较两个 Arc 是否指向同一份分配；
  - 不能比较 AddrSpace 内容是否相等，因为真正关心的是是否共享同一实例；
  - `other.aspace()` 会临时 clone 该进程的 Arc，比较结束后临时 Arc 被销毁。
- `})`
  - 结束 `any` 闭包和函数调用；
  - `any` 的结果就是函数最后一个表达式。
- `}`
  - 结束函数。

### 3. 把退出地址空间放入回收队列

源码：`kernel/src/task/ops.rs:112-117`

```rust
fn reclaim_process_address_space(proc_data: &ProcessData) {
    let aspace = proc_data.aspace();
    if !address_space_shared_with_live_process(&aspace, proc_data.proc.pid()) {
        crate::mm::defer_address_space_reclaim(aspace, crate::mm::AddrSpaceWriteOwner::ExitClear);
    }
}
```

逐行解释：

- `fn reclaim_process_address_space(proc_data: &ProcessData) {`
  - 函数是当前模块私有的，没有 `pub`；
  - `proc_data`：要退出的进程数据；
  - `&ProcessData`：只借用进程对象。
- `let aspace = proc_data.aspace();`
  - 再取得一个 Arc 强引用；
  - 这个 Arc 随后会被转移给队列。
- `if !address_space_shared_with_live_process(...) {`
  - `!`：只有“不再被其他活进程共享”才回收；
  - 第二次检查与前面的 locked-bytes 检查目的不同：前者做记账，这里决定能否
    真正删除映射。
- `defer_address_space_reclaim(...)`
  - `defer`：推迟；
  - `address_space`：被推迟回收的地址空间；
  - `reclaim`：实际释放；
  - `aspace` 不带 `&`，所有权移动到函数，再移动进队列；
  - `AddrSpaceWriteOwner::ExitClear`：为后续写锁探针贴上退出清理标签。
- `}`
  - 分别结束 if 和函数。

## 七、`exec clear` 当前代码逐行解释

源码：`kernel/src/syscall/task/execve.rs:275-300`

```rust
// Build the replacement image off to the side. This preserves the old
// image when loading fails and lets a vfork child stop sharing its parent's
// address space without cloning it first.
let mut new_aspace = new_user_aspace_empty()?;
copy_from_kernel(&mut new_aspace)?;
let (entry_point, user_stack_base) = if let Some(load_loc) = load_loc {
    load_user_app_from_location(&mut new_aspace, load_loc, path.as_str(), &args, &envs)?
} else {
    load_user_app(&mut new_aspace, Some(path.as_str()), &args, &envs)?
};
let page_table_root = new_aspace.page_table_root();
let old_aspace = proc_data.replace_aspace(Arc::new(crate::mm::AddrSpaceLock::new(new_aspace)));
let old_aspace_shared =
    address_space_shared_with_live_process(&old_aspace, proc_data.proc.pid());
let old_locked_bytes = if old_aspace_shared {
    0
} else {
    old_aspace.read().locked_bytes()
};
crate::syscall::release_process_mmap_resources(proc_data.proc.pid(), old_locked_bytes);
axtask::replace_current_page_table(page_table_root);
if old_aspace_shared {
    drop(old_aspace);
} else {
    defer_address_space_reclaim(old_aspace, AddrSpaceWriteOwner::ExecClear);
}
```

逐行解释：

- 三行注释：
  - `replacement image`：替换旧程序的全新进程映像；
  - `off to the side`：先在当前活动地址空间之外独立构建；
  - `preserves the old image when loading fails`：加载失败时保留旧映像；
  - `vfork child stop sharing`：vfork 子进程成功 exec 后脱离父地址空间；
  - `without cloning it first`：不需要先完整 fork 一份旧地址空间再替换。
- `let mut new_aspace = new_user_aspace_empty()?;`
  - `new_aspace`：新程序将使用的地址空间；
  - `new_user_aspace_empty`：创建只有基础页表结构的空用户地址空间；
  - `mut`：随后 ELF loader 会向其中加入 VMA/PTE；
  - `?`：分配失败立即结束 exec，旧地址空间仍未被替换。
- `copy_from_kernel(&mut new_aspace)?;`
  - `copy_from_kernel`：把每个用户地址空间都需要的内核映射复制进去；
  - `&mut`：函数将修改 new_aspace；
  - 此时仍没有取得 old_aspace 写锁。
- `let (entry_point, user_stack_base) = if let Some(load_loc) = load_loc {`
  - 左侧 tuple 一次绑定两个结果；
  - `entry_point`：新程序第一条用户指令地址；
  - `user_stack_base`：新用户栈基址；
  - `if let Some(load_loc)`：如果前面已解析出确定的可执行文件位置。
- `load_user_app_from_location(...)`
  - 从已经解析的 `load_loc` 装载；
  - `path.as_str()`：把 String 借用为 `&str`；
  - `&args`/`&envs`：程序参数和环境变量；
  - 返回 entry point 和 stack base。
- `} else {`
  - 没有预解析位置时进入普通路径。
- `load_user_app(&mut new_aspace, Some(path.as_str()), &args, &envs)?`
  - 由路径查找并装载程序；
  - `Some` 表示传入一个存在的可选路径。
- `};`
  - 结束装载分支并保存两个返回值。
- `let page_table_root = new_aspace.page_table_root();`
  - `page_table_root`：页表根物理地址；
  - 在移动 `new_aspace` 前先保存，后面用于切换硬件页表。
- `let old_aspace = proc_data.replace_aspace(...);`
  - `Arc::new`：给新 AddrSpaceLock 建立共享所有权；
  - `AddrSpaceLock::new(new_aspace)`：把新地址空间放入内层 BlockingRwLock；
  - `replace_aspace`：在外层 `RwLock<Arc<...>>` 写锁下交换指针；
  - `new_aspace` 的所有权移动进锁；
  - 返回值 `old_aspace` 是被替换掉的旧 Arc。
- `let old_aspace_shared =`
  - 绑定拆行书写，`=` 右侧在下一行。
- `address_space_shared_with_live_process(&old_aspace, proc_data.proc.pid());`
  - 检查 vfork 或 `CLONE_VM` 等场景下是否还有活进程使用旧 Arc；
  - `old_aspace_shared` 为 true 时绝不能 clear。
- `let old_locked_bytes = if old_aspace_shared {`
  - 准备释放旧进程 mmap/mlock 配额。
- `0`
  - 仍共享时不释放属于共享地址空间的锁页计数。
- `} else { old_aspace.read().locked_bytes() };`
  - 不共享时取旧地址空间读锁，读取完整锁页字节数。
- `release_process_mmap_resources(...)`
  - 更新进程级 mmap/mlock 外围记账；
  - 仍未进入 `ExecClear` 持锁区间。
- `axtask::replace_current_page_table(page_table_root);`
  - `axtask`：任务/调度子系统；
  - `replace_current_page_table`：让当前任务切到新页表根；
  - 这是安全顺序的关键：当前任务先停止使用旧用户页表，再把旧页表送去回收。
- `if old_aspace_shared {`
  - 分流共享与独占旧地址空间。
- `drop(old_aspace);`
  - 只减少当前持有的 Arc 引用；
  - 其他共享进程的 Arc 仍使对象存活；
  - 不执行主动 clear。
- `} else {`
  - 只有没有活共享者时进入回收路径。
- `defer_address_space_reclaim(old_aspace, AddrSpaceWriteOwner::ExecClear);`
  - 移动旧 Arc 到异步队列；
  - `ExecClear` 让后台取得写锁时使用 exec clear 探针。
- `}`
  - 结束分支。

## 八、exit 与 exec 共用的异步回收器逐行解释

### 1. 请求和全局状态

源码：`kernel/src/mm/aspace/mod.rs:346-352`

```rust
struct AddrSpaceReclaim {
    aspace: Arc<AddrSpaceLock>,
    owner: AddrSpaceWriteOwner,
}

static RECLAIM_QUEUE: SpinNoIrq<VecDeque<AddrSpaceReclaim>> = SpinNoIrq::new(VecDeque::new());
static RECLAIM_WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
```

逐行解释：

- `struct AddrSpaceReclaim {`
  - `AddrSpace`：待处理对象；
  - `Reclaim`：回收请求；
  - 结构体把“回收哪个地址空间”和“用什么标签统计”绑在一起。
- `aspace: Arc<AddrSpaceLock>,`
  - 队列持有强引用，保证任务退出后地址空间不会在 worker 处理前被析构；
  - Arc 也允许 ProcessData 等对象暂时仍保存引用。
- `owner: AddrSpaceWriteOwner,`
  - 保存 `ExitClear` 或 `ExecClear`；
  - worker 不需要根据调用栈猜来源。
- `}`
  - 请求结构结束。
- `static RECLAIM_QUEUE: ...`
  - `static`：整个内核运行期间只有一份；
  - `RECLAIM`：回收；
  - `QUEUE`：先进先出队列；
  - `SpinNoIrq`：持锁时关闭本 CPU 中断并使用自旋锁，保护很短的 push/pop；
  - `VecDeque`：双端队列，这里尾部 push、头部 pop；
  - `AddrSpaceReclaim`：队列元素类型；
  - `SpinNoIrq::new`：创建锁；
  - `VecDeque::new`：初始空队列。
- `static RECLAIM_WORKER_RUNNING: AtomicBool = AtomicBool::new(false);`
  - `WORKER`：后台工作任务；
  - `RUNNING`：当前是否已有 worker 负责队列；
  - `AtomicBool`：无需另一把锁即可原子读改布尔值；
  - 初始 false 表示没有 worker。

### 2. 入队和启动 worker

源码：`kernel/src/mm/aspace/mod.rs:354-385`

```rust
pub(crate) fn defer_address_space_reclaim(aspace: Arc<AddrSpaceLock>, owner: AddrSpaceWriteOwner) {
    RECLAIM_QUEUE
        .lock()
        .push_back(AddrSpaceReclaim { aspace, owner });
    if RECLAIM_WORKER_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }

    axtask::spawn(|| {
        loop {
            let request = { RECLAIM_QUEUE.lock().pop_front() };
            if let Some(request) = request {
                request.aspace.lock_profiled(request.owner).clear();
                continue;
            }

            RECLAIM_WORKER_RUNNING.store(false, Ordering::Release);
            if RECLAIM_QUEUE.lock().is_empty()
                || RECLAIM_WORKER_RUNNING
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
            {
                return;
            }
        }
    });
}
```

逐行解释：

- `pub(crate) fn defer_address_space_reclaim(...) {`
  - 当前 crate 的 exit 和 exec 模块可以调用；
  - `aspace` 按值接收，调用者的 Arc 所有权移入函数；
  - `owner` 决定探针标签。
- `RECLAIM_QUEUE`
  - 选中全局队列；链式调用拆成三行以便阅读。
- `.lock()`
  - 短暂取得队列自旋锁；
  - 不是 AddrSpace 写锁。
- `.push_back(AddrSpaceReclaim { aspace, owner });`
  - `push_back`：放到队尾，维持 FIFO；
  - 结构体字段简写分别移动 aspace 和复制 owner；
  - 语句结束时队列 guard 被销毁，队列锁释放。
- `if RECLAIM_WORKER_RUNNING.swap(true, Ordering::AcqRel) {`
  - `swap(true, ...)`：原子地把状态设成 true，并返回旧值；
  - `AcqRel`：acquire-release 内存顺序，既取得此前发布的状态，也发布当前修改；
  - 旧值为 true 表示已有 worker，此调用只需入队；
  - 旧值为 false 表示当前调用负责创建 worker。
- `return;`
  - 已有 worker 时立即返回退出/exec 路径；
  - 真正 clear 在后台进行。
- `axtask::spawn(|| {`
  - `spawn`：创建可由调度器运行的内核任务；
  - `||`：没有参数的闭包；
  - 闭包负责持续排空队列。
- `loop {`
  - 无限循环；只有队列确认为空且交接完成才 return。
- `let request = { RECLAIM_QUEUE.lock().pop_front() };`
  - `pop_front`：从队头取一个请求，返回 `Option<AddrSpaceReclaim>`；
  - 外层独立代码块很重要：块结束时队列 guard 立即释放；
  - 后面的 AddrSpace clear 不会持有全局队列自旋锁。
- `if let Some(request) = request {`
  - 有请求时取出结构体内容；
  - `None` 表示观察到队列为空。
- `request.aspace.lock_profiled(request.owner).clear();`
  - `request.aspace`：这次要清的具体 AddrSpaceLock；
  - `lock_profiled`：先取得该实例写锁，再启动 ExitClear/ExecClear 计时；
  - `request.owner`：对应标签；
  - `clear()`：通过 guard 的 `DerefMut` 调用 `AddrSpace::clear`；
  - 整条语句结束时临时 profiled guard 被销毁：先 unlock，再结束探针；
  - 当前代码对每个请求只执行一次，不再按 1 MiB 重新入队。
- `continue;`
  - 跳回 loop 顶部，处理下一请求；
  - 跳过下面的 worker 停止协议。
- `}`
  - 结束 `Some` 分支。
- `RECLAIM_WORKER_RUNNING.store(false, Ordering::Release);`
  - worker 观察到空队列，先宣布自己准备退出；
  - `Release` 保证此前处理完成的效果在其他 CPU 观察到 false 前可见。
- `if RECLAIM_QUEUE.lock().is_empty()`
  - 宣布 false 后重新检查队列；
  - 用于覆盖“入队者刚好和 worker 退出交错”的竞态。
- `|| RECLAIM_WORKER_RUNNING`
  - 如果队列非空，再检查是否已有另一个入队者接管了 worker 状态。
- `.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)`
  - 只有当前状态仍是 false 才改为 true；
  - 成功表示老 worker 自己继续处理刚到的请求；
  - 失败表示另一方已经设成 true 并会启动新 worker；
  - 第一个 Ordering 是成功时的顺序，第二个是失败时的顺序。
- `.is_err()`
  - compare-exchange 失败时为 true。
- 整体条件：
  - 队列仍空：return；
  - 队列非空但别人已接管：return；
  - 队列非空且当前 worker CAS 成功：条件为 false，继续 loop。
- `return;`
  - 安全退出 worker，不遗漏入队请求。
- 最后的 `}`、`});`、`}`
  - 分别结束 if、loop、闭包/spawn 调用和函数。

## 九、`clear()` 到底做了什么

### 1. `AddrSpace::clear`

源码：`kernel/src/mm/aspace/mod.rs:753-756`

```rust
/// Removes all mappings in the address space.
pub fn clear(&mut self) {
    self.areas.clear(&mut self.pt).unwrap();
}
```

逐行解释：

- 注释 `Removes all mappings in the address space`
  - `Removes`：删除；
  - `all mappings`：所有虚拟到物理映射；
  - `in the address space`：范围限于当前 AddrSpace。
- `pub fn clear(&mut self) {`
  - `clear`：清空；
  - `&mut self`：要求独占可变访问整个 AddrSpace；
  - 调用者通过写锁 guard 获得这种访问。
- `self.areas.clear(&mut self.pt).unwrap();`
  - `self.areas`：当前 AddrSpace 的 MemorySet/VMA 集合；
  - 第一个 `clear`：逐 VMA 删除底层映射，再清空 VMA 索引；
  - `&mut self.pt`：把同一 AddrSpace 的页表可变借给 MemorySet；
  - `unwrap`：如果 backend unmap 返回错误就 panic；
  - 当前设计把地址空间销毁错误视为不可恢复的内部一致性错误。
- `}`
  - 函数结束。

### 2. `MemorySet::clear`

源码：`vendor/memory_set/src/set.rs:216-223`

```rust
/// Remove all memory areas and the underlying mappings.
pub fn clear(&mut self, page_table: &mut B::PageTable) -> MappingResult {
    for (_, area) in self.areas.iter() {
        area.unmap_area(page_table)?;
    }
    self.areas.clear();
    Ok(())
}
```

逐行解释：

- 注释：
  - `memory areas`：VMA 元数据；
  - `underlying mappings`：页表项和 backend 维护的映射引用；
  - 两者都要清，不能只把 BTreeMap 丢掉。
- `pub fn clear(&mut self, page_table: &mut B::PageTable) -> MappingResult {`
  - `B`：泛型 mapping backend 类型；
  - `B::PageTable`：该 backend 关联的页表类型；
  - `MappingResult`：成功为 `Ok(())`，失败为 `MappingError`。
- `for (_, area) in self.areas.iter() {`
  - `self.areas`：通常是按 VMA 起始地址索引的 BTreeMap；
  - `iter()`：按键顺序遍历所有 VMA；
  - tuple 第一项是 key/起始地址；
  - `_`：key 在循环体中不用；
  - `area`：当前 MemoryArea/VMA；
  - 这里仍没有删除 BTree 节点，先保证 backend 清理成功。
- `area.unmap_area(page_table)?;`
  - `unmap_area`：删除整个 VMA 范围；
  - `?`：任何一个 VMA 失败就停止并向上返回错误；
  - 对大进程，这一行会反复进入不同 backend 的按页循环。
- `}`
  - VMA 循环结束。
- `self.areas.clear();`
  - 所有底层映射成功删除后，清空 VMA BTreeMap；
  - 这里只删除内存元数据节点，不再操作 PTE。
- `Ok(())`
  - 报告 clear 成功；
  - 无分号，所以是函数返回值。
- `}`
  - 函数结束。

### 3. `MemoryArea::unmap_area`

源码：`vendor/memory_set/src/area.rs:111-117`

```rust
/// Unmaps the whole memory area in the page table.
pub(crate) fn unmap_area(&self, page_table: &mut B::PageTable) -> MappingResult {
    self.backend
        .unmap(self.start(), self.size(), page_table)
        .then_some(())
        .ok_or(MappingError::BadState)
}
```

逐行解释：

- 注释：
  - `Unmaps`：解除映射；
  - `whole memory area`：不是子范围，而是整个 VMA；
  - `in the page table`：实际改变 PTE。
- `pub(crate) fn unmap_area(...) -> MappingResult {`
  - `unmap`：解除 virtual-to-physical mapping；
  - `area`：当前 MemoryArea；
  - `&self`：VMA 元数据本身不在这里修改；
  - 页表通过 `&mut` 修改。
- `self.backend`
  - 选中该 VMA 的实际 backend；
  - 可能是 `Cow`、`File`、`Shared` 或 `Linear`。
- `.unmap(self.start(), self.size(), page_table)`
  - `start()`：VMA 起始虚拟地址；
  - `size()`：VMA 字节长度；
  - backend 通过这两个值构造范围；
  - 返回 bool：true 成功，false 失败。
- `.then_some(())`
  - bool 为 true 时变成 `Some(())`；
  - bool 为 false 时变成 `None`。
- `.ok_or(MappingError::BadState)`
  - `Some(())` 变成 `Ok(())`；
  - `None` 变成 `Err(BadState)`；
  - `BadState` 表示 backend 或页表状态不一致。
- `}`
  - 函数结束。

### 4. 统一 Backend 分发层

源码：`kernel/src/mm/aspace/backend/mod.rs:269-278`

```rust
fn unmap(&self, start: VirtAddr, size: usize, pt: &mut PageTable) -> bool {
    let range = VirtAddrRange::from_start_size(start, size);
    let mut modify = pt.modify();
    if let Err(err) = BackendOps::unmap(self, range, &mut modify) {
        warn!("Failed to unmap area: {:?}", err);
        false
    } else {
        true
    }
}
```

逐行解释：

- `fn unmap(&self, start: VirtAddr, size: usize, pt: &mut PageTable) -> bool {`
  - 这是 `MappingBackend for Backend` 的统一入口；
  - `VirtAddr`：virtual address 类型；
  - `usize`：与机器字长相同的无符号整数；
  - `pt`：page table；
  - 返回 bool 适配 vendor/memory_set 的接口。
- `let range = VirtAddrRange::from_start_size(start, size);`
  - 把“起点 + 长度”规范化为 `[start, end)` 地址范围；
  - `VirtAddrRange`：虚拟地址范围类型。
- `let mut modify = pt.modify();`
  - 创建可修改页表的 cursor/guard；
  - `modify` 不是英语形容词，而是局部变量名；
  - 后续逐页 unmap 共用它，避免每一页重新建立顶层修改上下文。
- `if let Err(err) = BackendOps::unmap(self, range, &mut modify) {`
  - `BackendOps::unmap` 通过 enum-dispatch 调到 Cow/File/Shared/Linear 实现；
  - `Err(err)`：只在失败时进入 if；
  - `err`：具体 AxError/PagingError。
- `warn!(...)`
  - 输出警告；
  - `{:?}` 用 Debug 格式打印错误。
- `false`
  - 向 MemoryArea 报告失败。
- `} else { true }`
  - 没有错误则报告成功。
- `}`
  - 函数结束；`modify` 在此析构并结束页表修改上下文。

### 5. 最重的 COW unmap

源码：`kernel/src/mm/aspace/backend/cow.rs:774-799`

```rust
fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {
    debug!("Cow::unmap: {range:?}");
    for addr in pages_in(range, self.size)? {
        if let Ok((frame, _flags, page_size)) = pt.unmap(addr) {
            assert_eq!(page_size, self.size);
            let file_page = self.file_pages.remove(addr);
            pt.finish();
            if let Some(page) = file_page {
                if page.paddr() != frame {
                    warn!("unmapping COW page got mismatched cached frame");
                }
            } else {
                let frame_ref = frame_table(frame)
                    .lock()
                    .get_frame_ref(frame)
                    .ok_or(AxError::BadAddress)?;
                let mut frame_ref = frame_ref.lock();
                frame_ref.drop_frame(frame, self.size);
            }
            self.dec_mapped_pages();
        } else {
            // Deallocation is needn't if the page is not allocated.
        }
    }
    Ok(())
}
```

逐行解释：

- `fn unmap(&self, range: VirtAddrRange, pt: &mut PageTableMut) -> AxResult {`
  - `Cow` backend 的实际实现；
  - `COW`：copy on write，写时复制；
  - `PageTableMut`：可修改页表的操作上下文。
- `debug!("Cow::unmap: {range:?}");`
  - debug 级日志；
  - release/warn 配置下一般不会输出，但格式化路径仍由编译配置决定。
- `for addr in pages_in(range, self.size)? {`
  - `pages_in`：以 backend 页大小枚举范围中的每个虚拟页地址；
  - `self.size`：可能是 4 KiB，也可能是 backend 支持的更大页；
  - 即使范围内许多页没有 PTE，也会逐个地址调用 `pt.unmap`；
  - 这是稀疏大 VMA 可能耗时的重要原因。
- `if let Ok((frame, _flags, page_size)) = pt.unmap(addr) {`
  - `pt.unmap`：删除 addr 对应的 PTE；
  - 成功返回物理 `frame`、旧 flags 和 page size；
  - `_flags`：旧权限在回收中不用；
  - 未映射或其他错误都进入 else，目前不区分错误类型。
- `assert_eq!(page_size, self.size);`
  - 断言页表返回的页大小与 backend 预期一致；
  - 不一致说明内核内部映射状态损坏。
- `let file_page = self.file_pages.remove(addr);`
  - COW 区域可能来自文件页；
  - `file_pages` 是虚拟地址到缓存页引用的记录；
  - `remove` 同时查找并删除，返回 `Option<PageRef>`。
- `pt.finish();`
  - 完成本次页表修改所需的收尾；
  - `PageTableMut::finish` 会按已记录的地址刷新局部 TLB，或在记录升级为
    `ToFlush::Full` 时刷新整个 TLB，然后把 flush 状态恢复为 `None`；
  - 它位于逐页循环内，是需要继续测量的成本点。
- `if let Some(page) = file_page {`
  - 若该映射由文件缓存页引用支持，走文件页释放路径。
- `if page.paddr() != frame {`
  - `paddr`：physical address；
  - 验证反向记录中的物理页与刚从 PTE 删除的 frame 相同。
- `warn!(...)`
  - 不一致时记录警告，但仍继续清理。
- `} else {`
  - 没有 file-page 记录，说明这是匿名/COW frame。
- `let frame_ref = frame_table(frame)`
  - `frame_table`：按物理 frame 找到管理该 frame 的引用表。
- `.lock()`
  - 取得 frame table 锁；
  - 这是 AddrSpace 写锁内部的另一把锁。
- `.get_frame_ref(frame)`
  - 查找具体 frame 的共享引用对象。
- `.ok_or(AxError::BadAddress)?`
  - 查不到就把 None 转成 BadAddress，并向上返回错误。
- `let mut frame_ref = frame_ref.lock();`
  - 再取得该 frame 引用对象自己的锁；
  - 允许修改共享计数/所有权状态。
- `frame_ref.drop_frame(frame, self.size);`
  - `drop_frame`：减少当前映射对物理页的引用；
  - 引用降到零时才真正归还物理页；
  - `self.size` 用于正确处理页大小。
- `self.dec_mapped_pages();`
  - `dec`：decrement，递减；
  - 更新该 CowBackend 的已映射页计数。
- `} else {`
  - `pt.unmap` 没成功时进入；
  - 当前代码把“本来就没分配”作为正常情况跳过。
- 注释 `Deallocation is needn't...`
  - 意思是页面未分配时无需释放；
  - 英文语法不自然，但代码语义就是 no-op。
- `}`
  - 结束逐页循环。
- `Ok(())`
  - 全部页处理完成。
- `}`
  - 函数结束。

### 6. File、Shared 和 Linear 的差别

| backend | clear 时的核心动作 | 是否逐虚拟页 |
|---|---|---|
| `CowBackend` | 删除 PTE、删除 file-page 引用或递减 frame 引用 | 是 |
| `FileBackend` | 删除 PTE，并从 `mapped_pages` 反向表删除地址 | 是，固定 4 KiB |
| `SharedBackend` | 删除 PTE，物理页由共享 `Arc<SharedPages>` 管理 | 是 |
| `LinearBackend` | 调用 `unmap_region` 删除整段线性映射 | 接口上按 region |

因此 `MemorySet::clear` 不是简单的 `BTreeMap::clear`。真正的大头通常发生在
清空 BTreeMap 之前的 backend 按页遍历、PTE 删除和引用计数维护。

## 十、`clone` 当前代码逐行解释

### 1. 哪些 clone 根本不会复制地址空间

源码：`kernel/src/syscall/task/clone.rs:197-226`

```rust
let new_proc_data = if flags.contains(CloneFlags::THREAD) {
    new_task
        .ctx_mut()
        .set_page_table_root(old_proc_data.aspace().read().page_table_root());
    old_proc_data.clone()
} else {
    // 创建新 Process，省略与本节无关的代码
    let aspace = if flags.contains(CloneFlags::VM) {
        old_proc_data.aspace()
    } else {
        let old_aspace = old_proc_data.aspace();
        let mut aspace = old_aspace.lock_profiled(AddrSpaceWriteOwner::Clone);
        let aspace = aspace.try_clone()?;
        {
            let mut child_aspace = aspace.lock_profiled(AddrSpaceWriteOwner::Clone);
            copy_from_kernel(&mut child_aspace)?;
            crate::syscall::inherit_wipeonfork_ranges(
                old_proc_data.proc.pid(),
                tid,
                &mut child_aspace,
            )?;
        }
        aspace
    };
}
```

逐行解释：

- `let new_proc_data = if flags.contains(CloneFlags::THREAD) {`
  - `new_proc_data`：新任务最终关联的 ProcessData；
  - `flags`：clone 选项集合；
  - `contains`：是否包含指定 bit；
  - `CloneFlags::THREAD`：Linux 的 `CLONE_THREAD`，新任务进入同一线程组；
  - 同一线程组直接共享整个 ProcessData。
- `new_task`
  - 新建但尚未正式调度的任务对象。
- `.ctx_mut()`
  - `ctx`：CPU/user context；
  - `mut`：取得可修改上下文。
- `.set_page_table_root(...)`
  - 把父进程当前页表根写入新线程上下文；
  - 线程共享地址空间，不创建新页表。
- `old_proc_data.aspace().read().page_table_root()`
  - `aspace()`：clone 父进程 Arc；
  - `read()`：短暂取得 AddrSpace 读锁；
  - `page_table_root()`：读取页表根；
  - 这条路径没有 `AddrSpaceWriteOwner::Clone`。
- `old_proc_data.clone()`
  - clone 的是 ProcessData 的 Arc；
  - 新线程与旧线程共享同一个进程对象。
- `} else {`
  - 不带 `CLONE_THREAD`，要创建独立 ProcessData。
- `let aspace = if flags.contains(CloneFlags::VM) {`
  - `CloneFlags::VM` 对应 `CLONE_VM`；
  - 含义是新旧进程共享同一虚拟地址空间。
- `old_proc_data.aspace()`
  - 只 clone Arc；
  - 不复制 VMA/PTE；
  - vfork 通常使用这种共享方式。
- `} else {`
  - 独立进程且不共享 VM，进入真正的 fork 地址空间复制热点。
- `let old_aspace = old_proc_data.aspace();`
  - `old_aspace`：父进程的 `Arc<AddrSpaceLock>`；
  - Arc 使父地址空间在 clone 完成前保持存活。
- `let mut aspace = old_aspace.lock_profiled(AddrSpaceWriteOwner::Clone);`
  - 此处局部变量 `aspace` 的类型其实是
    `ProfiledAddrSpaceWriteGuard`，不是 Arc；
  - `mut`：`try_clone` 需要 `&mut self`；
  - 取得父地址空间写锁；
  - 从这一行成功返回起开始 Clone hold 计时。
- `let aspace = aspace.try_clone()?;`
  - 右侧 `aspace`：上一行的父写锁 guard；
  - `try_clone()`：复制父 VMA/PTE，返回子 `Arc<AddrSpaceLock>`；
  - 左侧新的 `aspace`：子地址空间 Arc；
  - 这是 Rust 变量遮蔽：新绑定与旧绑定同名，但类型不同；
  - `?`：复制失败则退出 do_clone。
- 这里最容易误读：
  - 被遮蔽的父 guard **不会在这一行立即 drop**；
  - Rust 局部变量通常在其声明所在作用域结束时按逆序销毁；
  - 所以父写锁继续保持到这个 `else { ... }` 分支结束；
  - 后面的子锁、`copy_from_kernel`、wipe-on-fork 处理仍位于父写锁时间内。
- 单独的 `{`
  - 创建更短的内部作用域；
  - 用于确保 `child_aspace` guard 在 line 224 对应的 `}` 处释放。
- `let mut child_aspace = aspace.lock_profiled(AddrSpaceWriteOwner::Clone);`
  - 此处 `aspace` 是子 Arc；
  - 取得子地址空间写锁；
  - 又创建一个 Clone profile interval；
  - 父 Clone interval 仍在外层存活，因此两个区间嵌套并被累计两次。
- `copy_from_kernel(&mut child_aspace)?;`
  - 给子地址空间补齐内核映射；
  - 使用子写锁；
  - 当前父写锁也仍被持有，但此操作主要修改子地址空间。
- `inherit_wipeonfork_ranges(`
  - `inherit`：继承；
  - `wipe`：清零；
  - `onfork`：fork 时；
  - 处理 `MADV_WIPEONFORK` 元数据和子映射。
- `old_proc_data.proc.pid(),`
  - 父 PID，用来查父进程登记的 wipe-on-fork 范围。
- `tid,`
  - 新任务 ID，在新进程场景也作为新 PID 使用。
- `&mut child_aspace,`
  - 修改子地址空间。
- `)?;`
  - 调用结束；失败则回滚 do_clone；
  - `?` 传播错误。
- 内层 `}`
  - `child_aspace` guard 在这里销毁；
  - 子写锁释放，对应的第二/第三个 Clone interval 结束。
- `aspace`
  - 尾表达式，把子 Arc 返回给外层 `let aspace = if ...`；
  - 没有分号，所以不是丢弃值。
- 外层 `}`
  - 退出不带 CLONE_VM 的分支；
  - 被遮蔽但仍存活的父 guard 此时才销毁，父写锁和 profile interval 结束。
- `};`
  - 结束 if 表达式，外层 `aspace` 现在统一是子或共享地址空间 Arc。

这个变量遮蔽是当前最直接的可缩短锁区间点。保持现有语义的最小结构应当是：

```rust
let aspace = {
    let mut parent_aspace = old_aspace.lock_profiled(AddrSpaceWriteOwner::Clone);
    parent_aspace.try_clone()?
}; // 父写锁在这里明确释放

{
    let mut child_aspace = aspace.lock_profiled(AddrSpaceWriteOwner::Clone);
    copy_from_kernel(&mut child_aspace)?;
    // 处理 wipe-on-fork
}
```

这段只是说明锁生命周期边界，不代表本文已经修改代码。它不能解决
`try_clone()` 本身在父写锁内逐 VMA、逐页扫描的问题。

### 2. `AddrSpace::try_clone` 逐行

源码：`kernel/src/mm/aspace/mod.rs:1016-1053`

```rust
/// Attempts to clone the current address space into a new one.
///
/// This method creates a new empty address space with the same base and
/// size, then iterates over all memory areas in the original address
/// space to copy or share their mappings into the new one.
pub fn try_clone(&mut self) -> AxResult<Arc<AddrSpaceLock>> {
    let new_aspace = Arc::new(AddrSpaceLock::new(Self::new_empty(
        self.base(),
        self.size(),
    )?));
    let new_aspace_clone = new_aspace.clone();

    let mut guard = new_aspace.lock_profiled(AddrSpaceWriteOwner::Clone);

    let mut self_modify = self.pt.modify();
    for area in self.areas.iter() {
        let new_backend = area.backend().clone_map(
            area.va_range(),
            area.flags(),
            &mut self_modify,
            &mut guard.pt.modify(),
            &new_aspace_clone,
        )?;

        let new_area = MemoryArea::new_with_metadata(
            area.start(),
            area.size(),
            area.flags(),
            new_backend,
            area.metadata().for_fork(),
        );
        let aspace = guard.deref_mut();
        aspace.areas.map(new_area, &mut aspace.pt, false)?;
    }
    drop(guard);

    Ok(new_aspace)
}
```

逐行解释：

- 四行文档注释：
  - `Attempts`：尝试，表示可能返回错误；
  - `clone`：复制；
  - `current address space`：`self`，即父地址空间；
  - `new empty address space`：先建空壳；
  - `same base and size`：用户虚拟范围边界相同；
  - `iterates over all memory areas`：遍历所有 VMA；
  - `copy or share`：不同 backend 选择复制或共享物理页。
- `pub fn try_clone(&mut self) -> AxResult<Arc<AddrSpaceLock>> {`
  - `try_`：命名约定，操作可能失败；
  - `clone`：这里不是 Rust trait 的浅 clone，而是地址空间 fork；
  - `&mut self`：要求父 AddrSpace 独占可变借用；
  - 调用处用父写锁满足；
  - 返回子地址空间的 Arc。
- `let new_aspace = Arc::new(AddrSpaceLock::new(Self::new_empty(`
  - 从里向外读：
    1. `Self::new_empty` 创建空 AddrSpace；
    2. `AddrSpaceLock::new` 加内层读写锁；
    3. `Arc::new` 加共享所有权；
  - `new_aspace`：最终子地址空间 Arc。
- `self.base(),`
  - 父用户地址空间起点。
- `self.size(),`
  - 父用户地址空间跨度。
- `)?));`
  - `?` 传播创建页表失败；
  - 三层右括号依次关闭 `new_empty`、`AddrSpaceLock::new`、`Arc::new`。
- `let new_aspace_clone = new_aspace.clone();`
  - 再持有一份子 Arc；
  - 后面传给 FileBackend，让文件缓存 eviction listener 能弱引用/关联新地址空间；
  - 不复制 AddrSpace 内容。
- `let mut guard = new_aspace.lock_profiled(AddrSpaceWriteOwner::Clone);`
  - 取得子 AddrSpace 写锁；
  - `guard` 允许修改子 VMA 和子页表；
  - 这是一个嵌套在父 Clone 写锁内的 profile interval。
- `let mut self_modify = self.pt.modify();`
  - `self`：父 AddrSpace；
  - `pt`：父页表；
  - `modify()`：创建父页表可修改上下文；
  - COW clone 需要去掉父 PTE 的 WRITE 位，所以不能只读；
  - 该上下文覆盖整个 VMA 循环。
- `for area in self.areas.iter() {`
  - 遍历父地址空间全部 VMA；
  - `area` 是当前父 MemoryArea 的共享引用；
  - 父写锁确保循环期间 VMA 集合不被 mmap/munmap 改变。
- `let new_backend = area.backend().clone_map(`
  - `area.backend()`：取当前 VMA 的 Cow/File/Shared/Linear backend；
  - `clone_map`：为另一个页表复制此映射语义；
  - 返回值是子 VMA 要保存的新 Backend。
- `area.va_range(),`
  - 当前 VMA 的完整 `[start, end)` 范围。
- `area.flags(),`
  - 当前 VMA 权限，如 READ/WRITE/EXEC/USER。
- `&mut self_modify,`
  - 父页表修改上下文；
  - CowBackend 会用它把父可写 PTE 改为只读 COW。
- `&mut guard.pt.modify(),`
  - `guard.pt`：子页表；
  - `modify()`：为当前 VMA 创建子页表修改上下文；
  - 前导 `&mut` 借用这个临时上下文；
  - CowBackend 用它建立子 PTE。
- `&new_aspace_clone,`
  - 借用子 Arc；
  - FileBackend 用于注册 page-cache eviction 监听；
  - Cow/Shared/Linear 参数名前带 `_`，表示不使用。
- `)?;`
  - clone_map 失败时立即传播；
  - 已经创建的部分子映射依赖对象析构/局部 rollback 清理。
- `let new_area = MemoryArea::new_with_metadata(`
  - 创建子 VMA 元数据对象；
  - 此时还没插入子 MemorySet。
- `area.start(),`
  - 子 VMA 与父 VMA 起点一致。
- `area.size(),`
  - 长度一致。
- `area.flags(),`
  - VMA 级 flags 初始一致；
  - CowBackend 已单独把驻留 PTE 的 WRITE 位去掉。
- `new_backend,`
  - 移动 clone_map 返回的 backend 到新 VMA。
- `area.metadata().for_fork(),`
  - `metadata()`：父 VMA 附加元数据；
  - `for_fork()`：按 fork 语义复制，例如处理 may_flags 等；
  - 不是简单 bitwise copy 的命名意图。
- `);`
  - 完成 new_area 构造。
- `let aspace = guard.deref_mut();`
  - 把自定义 guard 显式解引用成 `&mut AddrSpace`；
  - 这样可同时访问 `areas` 和 `pt` 两个字段；
  - 此处局部 `aspace` 只是可变引用，不是 Arc，也不是父 guard。
- `aspace.areas.map(new_area, &mut aspace.pt, false)?;`
  - `areas.map`：把 new_area 安装进子 MemorySet；
  - `new_area` 所有权移入；
  - `&mut aspace.pt`：若 backend 的 map 需要，更新子页表；
  - `false`：遇到重叠不自动 unmap 旧区域；新地址空间应为空，不允许重叠；
  - `?`：安装失败向上返回。
- `}`
  - 结束所有父 VMA 的循环。
- `drop(guard);`
  - 显式释放子写锁；
  - 子 Clone profile interval 在这里结束；
  - 之所以显式 drop，是后面要移动 `new_aspace` 返回。
- `Ok(new_aspace)`
  - 成功返回子 Arc；
  - Arc 被移动给调用者。
- `}`
  - 函数结束；父页表 `self_modify` 在此析构。

### 3. COW `clone_map` 为什么是 clone 的核心慢路径

源码：`kernel/src/mm/aspace/backend/cow.rs:837-896`

```rust
fn clone_map(
    &self,
    range: VirtAddrRange,
    flags: MappingFlags,
    old_pt: &mut PageTableMut,
    new_pt: &mut PageTableMut,
    _new_aspace: &Arc<AddrSpaceLock>,
) -> AxResult<Backend> {
    let new_backend = self.clone_for_fork();
    if self.mapped_pages.load(Ordering::Relaxed) == 0 {
        return Ok(Backend::Cow(new_backend));
    }

    let cow_flags = flags - MappingFlags::WRITE;

    for vaddr in pages_in(range, self.size)? {
        match old_pt.query(vaddr) {
            Ok((paddr, _, page_size)) => {
                assert_eq!(page_size, self.size);
                let file_page = self.file_pages.get(vaddr);
                let result = if let Some(page) = file_page {
                    if page.paddr() != paddr {
                        Err(AxError::BadAddress)
                    } else {
                        (|| -> AxResult {
                            old_pt.protect(vaddr, cow_flags)?;
                            new_pt.map(vaddr, paddr, self.size, cow_flags)?;
                            new_backend.file_pages.insert(vaddr, page);
                            Ok(())
                        })()
                    }
                } else {
                    self.share_existing_at(vaddr, paddr, cow_flags, old_pt, new_pt)
                };
                if let Err(err) = result {
                    new_backend
                        .rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                    return Err(err);
                }
                new_backend.mapped_pages.fetch_add(1, Ordering::Relaxed);
            }
            Err(PagingError::NotMapped) => {}
            Err(_) => {
                new_backend
                    .rollback_shared_range(VirtAddrRange::new(range.start, vaddr), new_pt);
                return Err(AxError::BadAddress);
            }
        };
    }

    Ok(Backend::Cow(new_backend))
}
```

逐行解释：

- `fn clone_map(`
  - clone 当前 backend 的“跨页表映射”；
  - 与普通 `Clone::clone` 浅复制 Rust 对象不同。
- `&self,`
  - 当前父 CowBackend。
- `range: VirtAddrRange,`
  - 当前父 VMA 的完整虚拟范围。
- `flags: MappingFlags,`
  - VMA 权限。
- `old_pt: &mut PageTableMut,`
  - 父页表可修改上下文；
  - 名字 `old` 表示父/原页表。
- `new_pt: &mut PageTableMut,`
  - 子页表可修改上下文。
- `_new_aspace: &Arc<AddrSpaceLock>,`
  - CowBackend 当前不使用子 Arc；
  - 前导 `_` 抑制未使用参数警告；
  - 参数保留是为了统一 BackendOps trait 签名。
- `) -> AxResult<Backend> {`
  - 返回一个可放入子 VMA 的统一 Backend enum。
- `let new_backend = self.clone_for_fork();`
  - 建立子 CowBackend 元数据；
  - `for_fork` 表示按 fork 语义初始化，不是复制所有映射状态。
- `if self.mapped_pages.load(Ordering::Relaxed) == 0 {`
  - `mapped_pages`：此 CowBackend 当前驻留 PTE 数量计数；
  - `load`：原子读取；
  - `Relaxed`：只要求原子性，不用它建立跨线程 happens-before；
  - 为 0 表示 lazy VMA 尚未真正映射任何页。
- `return Ok(Backend::Cow(new_backend));`
  - 没有驻留页时无需扫描页表；
  - 直接返回子 CowBackend；
  - 这是已有的重要 fast path。
- `}`
  - fast path 结束。
- `let cow_flags = flags - MappingFlags::WRITE;`
  - `cow_flags`：COW 状态下 PTE 使用的权限；
  - `- MappingFlags::WRITE`：从 bitflags 中移除 WRITE 位；
  - 父子都必须暂时只读，后续写缺页才复制。
- `for vaddr in pages_in(range, self.size)? {`
  - `vaddr`：当前 virtual address；
  - 以 backend 页大小遍历**整个 VMA 虚拟范围**；
  - 即使 `mapped_pages` 只有 1，只要不为 0，大 VMA 的所有虚拟页都会 query；
  - 这是 clone 在稀疏映射上放大的关键复杂度。
- `match old_pt.query(vaddr) {`
  - 查询父页表该地址是否有 PTE；
  - `match` 分成功、未映射和其他错误。
- `Ok((paddr, _, page_size)) => {`
  - `paddr`：父 PTE 指向的物理地址；
  - `_`：忽略原 PTE flags，因为将使用 VMA 的 cow_flags；
  - `page_size`：实际 PTE 页大小。
- `assert_eq!(page_size, self.size);`
  - 保证页表大小与 backend 步长一致。
- `let file_page = self.file_pages.get(vaddr);`
  - 私有文件映射也可能使用 CowBackend；
  - 查找当前地址是否对应 page-cache 页引用。
- `let result = if let Some(page) = file_page {`
  - 统一把“文件页共享”和“匿名 frame 共享”的结果保存为 AxResult；
  - `page` 是已有文件页引用。
- `if page.paddr() != paddr {`
  - 验证 page-cache 记录与父 PTE 指向同一物理页。
- `Err(AxError::BadAddress)`
  - 不一致时构造错误，稍后走 rollback。
- `} else {`
  - 物理地址一致，开始建立父子 COW PTE。
- `(|| -> AxResult {`
  - 创建并立刻调用一个无参数闭包；
  - 显式返回 `AxResult`，使内部可以使用 `?`。
- `old_pt.protect(vaddr, cow_flags)?;`
  - 修改父 PTE 权限；
  - 去掉 WRITE 是 COW 正确性的关键；
  - 如果父仍可写，它可能绕过写缺页直接修改共享物理页。
- `new_pt.map(vaddr, paddr, self.size, cow_flags)?;`
  - 在子页表同一虚拟地址建立映射；
  - 指向同一 paddr；
  - 同样使用只读 cow_flags。
- `new_backend.file_pages.insert(vaddr, page);`
  - 在子 backend 记录同一文件缓存页引用；
  - `insert` 增加/转移相应共享引用关系。
- `Ok(())`
  - 文件页 COW 建立成功。
- `})()`
  - 第一个 `}` 结束闭包；
  - 后面的 `()` 立即执行闭包。
- `} else {`
  - 当前映射不是登记的文件页，按匿名 COW frame 处理。
- `self.share_existing_at(vaddr, paddr, cow_flags, old_pt, new_pt)`
  - `share`：共享；
  - `existing`：已经存在的父物理 frame；
  - `at`：指定虚拟地址；
  - 函数负责父 PTE protect、子 PTE map 和 frame 引用计数增加。
- `};`
  - 两种路径的 AxResult 统一绑定为 `result`。
- `if let Err(err) = result {`
  - 当前页建立失败时开始回滚之前已成功处理的页。
- `new_backend.rollback_shared_range(...)`
  - `rollback`：撤销；
  - `shared_range`：已经在子页表共享成功的前缀；
  - 避免错误返回时泄漏子 PTE 或 frame 引用。
- `VirtAddrRange::new(range.start, vaddr)`
  - 回滚范围从 VMA 起点到当前失败地址，不包含当前页。
- `return Err(err);`
  - 回滚后把原错误返回。
- `new_backend.mapped_pages.fetch_add(1, Ordering::Relaxed);`
  - 当前页成功后，子 backend 驻留页计数加 1；
  - `fetch_add` 返回旧值，但此处忽略；
  - Relaxed 只保证计数更新原子。
- `}`
  - 成功映射分支结束。
- `Err(PagingError::NotMapped) => {}`
  - 父页表没有这个虚拟页；
  - lazy/sparse 页直接跳过；
  - 问题是仍为它付出了一次 `query`。
- `Err(_) => {`
  - 其他页表查询错误；
  - `_` 忽略具体 PagingError。
- `rollback_shared_range(...)`
  - 回滚此前成功页，与映射失败路径相同。
- `return Err(AxError::BadAddress);`
  - 对外统一报告坏地址。
- `};`
  - 结束 match；分号丢弃 match 的 unit 值。
- `}`
  - 结束整个 VMA 的逐页循环。
- `Ok(Backend::Cow(new_backend))`
  - 所有驻留父页都已共享后，返回子 CowBackend。
- `}`
  - 函数结束。

## 十一、三项热点之间最大的区别

| 维度 | `exit clear` | `exec clear` | `clone` |
|---|---|---|---|
| 操作对象 | 退出进程的旧地址空间 | 被新映像替换的旧地址空间 | 仍活跃的父地址空间和新子地址空间 |
| 当前执行位置 | 后台单 worker | 后台单 worker | 发起 clone 的任务同步执行 |
| 是否先检查共享者 | 是 | 是 | 由 `CLONE_VM` flags 决定共享或复制 |
| 是否修改父/旧 PTE | 删除旧 PTE | 删除旧 PTE | 把父 PTE 改为 COW，并建立子 PTE |
| 长锁是否直接阻塞活缺页 | 通常不会 | 切页表后通常不会 | **会** |
| 主要复杂度 | 全 VMA、全驻留页回收 | 全 VMA、全驻留页回收 | 全 VMA，COW VMA 还可能扫完整虚拟范围 |
| 当前最明显的额外开销 | 单 worker 及逐页引用清理 | 同左 | 父锁生命周期过宽、嵌套统计、稀疏范围扫描 |

## 十二、从当前源码能直接得出的优化顺序

### 第一优先：先缩短 clone 已经不必要的父锁覆盖范围

在不改变 COW 算法前，先显式结束父 guard，再执行只修改子地址空间的
`copy_from_kernel` 和 `inherit_wipeonfork_ranges`。这项修改：

- 不改变父 PTE 的 COW 正确性；
- 不要求新的并发页表协议；
- 能从父 AddrSpace 写锁中移出明确的子空间工作；
- 还会让 Clone 探针的父/子区间边界更容易解释。

### 第二优先：clone 按驻留 PTE 工作，不扫描稀疏 VMA 全范围

当前只用 `mapped_pages == 0` 做全空 fast path。一旦计数非零，
`pages_in(range, self.size)` 仍遍历整个 VMA。下一步需要让 CowBackend 能迭代
实际登记的驻留地址，或者由页表提供 present-PTE range iterator。

必须保证：

- 遍历期间父 VMA/PTE 不被并发 munmap；
- 每个父可写 PTE 在子任务可运行前都已变为 COW；
- 失败 rollback 正确递减已经增加的 frame/file-page 引用。

### 第三优先：把 clone 拆成 prepare/execute/commit

目标流程应是：

```text
prepare（短父锁）
    记录 VMA generation、实际驻留映射和 backend 引用

execute（锁外）
    分配子页表节点、构造子 VMA/backend、准备 PTE 更新计划

commit（短父写锁）
    校验 generation
    批量把父 PTE 改成 COW
    安装子 PTE/MemorySet

stale
    generation 已变化则有限次数重新 prepare
```

不能直接把现有 `clone_map` 全部挪到锁外，因为 `old_pt.protect` 会改变活父进程
页表；也不能先让子任务运行再慢慢 protect 父 PTE，否则父子可能同时以 WRITE
权限修改同一物理页。

### 第四优先：为死亡地址空间建立可转移的回收对象

当前 exit/exec 已经把 clear 移出同步路径，但 worker 仍要取得
`AddrSpaceLock`。最终可以把：

```text
MemorySet + 用户页表所有权 + backend 清理状态
```

在短临界区内整体移动到 `ReclaimImage`，再完全锁外清理。前提是：

- exec 已切到新页表；
- exit 当前任务不会再回用户态；
- 旧页表在最后一个使用它的 CPU 离开前不能释放；
- COW frame、file mapped_pages 和共享页引用必须各减一次。

## 十三、读这份数据时不能得出的结论

1. 不能说“exit clear 阻塞了 306.57 秒的所有 CPU”。这是累计持锁 task-time，
   而且操作的大多是不同的死亡地址空间。
2. 不能说“发生了 393,719 次进程退出”。那是旧分批实现的写锁批次数。
3. 不能说“发生了 1,552 次 fork”。那是 1/16 抽样后估算的 Clone 写锁区间，
   一次 fork 可以产生多个、甚至嵌套的区间。
4. 不能把 exit、exec、clone 三行相加当墙钟。不同 AddrSpace 可并行，Clone
   内部父子事件还会嵌套。
5. 不能因为 clear 锁很长就继续把死亡地址空间按固定虚拟字节切批。上一版完整
   测量已经证明这种做法让正式编译退化约 4%。

最可靠的当前结论是：

```text
clear 的主要成本 = 逐 VMA + 逐虚拟页/PTE + backend 引用清理

clone 的主要成本 = 父写锁内遍历全部 VMA
                  + 稀疏 COW VMA 的全范围页表查询
                  + 父 PTE protect
                  + 子 PTE map
                  + 当前过宽的父 guard 生命周期
```

## 附录：表中几十万 clear 次数对应的历史代码

本附录只用于解释这次完整测量。它来自提交 `965bd66e`，已在当前
`7ad5cc6b` 中删除，不能把它误认成当前实现。

历史 worker 核心：

```rust
const RECLAIM_BATCH_PAGES: usize = 256;

let mut aspace = request.aspace.lock_profiled(request.owner);
let finished = aspace.clear_batch(RECLAIM_BATCH_PAGES);
drop(aspace);
if !finished {
    RECLAIM_QUEUE.lock().push_back(request);
    axtask::yield_now();
}
```

逐行解释：

- `const RECLAIM_BATCH_PAGES: usize = 256;`
  - `const`：编译期常量；
  - `RECLAIM`：回收；
  - `BATCH`：一批；
  - `PAGES`：以 4 KiB 基页数量表达；
  - `usize`：机器字宽无符号整数；
  - `256`：一批最多 256 个基页，即 1 MiB。
- `let mut aspace = request.aspace.lock_profiled(request.owner);`
  - 取得当前请求的 AddrSpace 写锁；
  - `mut` 用于调用会修改 AddrSpace 的 `clear_batch`；
  - 每执行到这里一次就产生一个 ExitClear 或 ExecClear 持锁区间。
- `let finished = aspace.clear_batch(RECLAIM_BATCH_PAGES);`
  - `finished`：本批结束后整个地址空间是否已经清空；
  - `clear_batch`：只清一段；
  - 传入 256 页上限。
- `drop(aspace);`
  - 显式释放本批写锁并结束 profile interval。
- `if !finished {`
  - 地址空间仍有 VMA 时继续。
- `RECLAIM_QUEUE.lock().push_back(request);`
  - 把同一个请求重新放到全局队尾；
  - 以后会再次取得同一 AddrSpace 写锁。
- `axtask::yield_now();`
  - 主动让出当前 CPU；
  - 希望其他任务运行，但也增加一次调度循环。
- `}`
  - 未完成分支结束。

历史 `clear_batch`：

```rust
fn clear_batch(&mut self, max_pages: usize) -> bool {
    let Some((start, size, page_size)) = self.areas.iter().next().map(|area| {
        (
            area.start(),
            area.size(),
            area.backend().page_size() as usize,
        )
    }) else {
        return true;
    };
    let batch_size = max_pages
        .saturating_mul(PAGE_SIZE_4K)
        .max(page_size)
        .min(size);
    self.areas.unmap(start, batch_size, &mut self.pt).unwrap();
    self.areas.is_empty()
}
```

逐行解释：

- `fn clear_batch(&mut self, max_pages: usize) -> bool {`
  - `clear_batch`：清理一批；
  - `max_pages`：最多基页数；
  - 返回 bool 表示是否全部完成。
- `let Some((start, size, page_size)) = ... else {`
  - 使用 let-else 解构第一个 VMA 的起点、长度和 backend 页大小；
  - 没有第一个 VMA就进入 else。
- `self.areas.iter().next()`
  - 每批都重新从 MemorySet/BTreeMap 开头建立迭代器并取第一个 VMA。
- `.map(|area| {`
  - 如果有 VMA，把 `area` 转换成三元 tuple。
- `area.start(),`
  - 当前第一个 VMA 起点。
- `area.size(),`
  - 当前 VMA 剩余长度；
  - 上一批 shrink 后，这个值会变化。
- `area.backend().page_size() as usize,`
  - backend 的自然页大小；
  - `as usize` 转成字节计算使用的整数类型。
- `}) else { return true; };`
  - `next()` 为 None 表示没有 VMA，整个回收完成。
- `let batch_size = max_pages`
  - 开始计算本批虚拟字节长度。
- `.saturating_mul(PAGE_SIZE_4K)`
  - `saturating_mul`：乘法溢出时钳到最大值，不回绕；
  - `PAGE_SIZE_4K`：4096；
  - 正常输入得到 256 × 4096。
- `.max(page_size)`
  - 至少覆盖 backend 的一个自然页；
  - 避免大页 backend 得到比一个页还小的范围。
- `.min(size);`
  - 不能超过当前 VMA 剩余长度。
- `self.areas.unmap(start, batch_size, &mut self.pt).unwrap();`
  - 从第一个 VMA 起点 unmap 这一批；
  - 如果只删除 VMA 左侧，会进入 `shrink_left`；
  - 因而同一大 VMA会被反复 remove、缩短、重新插入 BTreeMap；
  - `unwrap` 把内部错误升级为 panic。
- `self.areas.is_empty()`
  - 本批之后检查 VMA 集合是否为空；
  - 无分号，因此是返回值。
- `}`
  - 函数结束。

这段历史代码把“写锁一次持有很久”改成“同一死亡地址空间取得几十万次短写锁”，
但没有减少总 PTE、backend 引用和物理页清理量，反而增加 BTree、队列和调度
成本。这正是表中 393,719/708,143 的来源，也是当前代码将其删除的原因。

## 十四、Linux 6.12.96 的对应实现

本节对照本机仓库 `/home/wuxun/Projects/linux-grader-baseline` 中的 Linux
6.12.96，当前源码提交为 `867a3598`。这里的“Linux”只指这一份实际用于
对比测试的源码，不用印象或其他内核版本代替。

先给结论：

1. Linux 的 fork **也会取得父进程的地址空间写锁**；
2. 所以 Linux 快的原因不是“完全没有大锁”；
3. Linux 在锁内做的工作远少于 Moss 当前实现；
4. Linux 按页表层级跳过没有 PTE 的大空洞；
5. Linux 对能够通过缺页重新建立的映射直接跳过 PTE 复制；
6. Linux 按 PTE 页和 folio 批量处理引用计数；
7. Linux 用 `mmu_gather` 将“清 PTE”“刷 TLB”“释放物理页”分成有序的批量阶段；
8. Moss 当前最值得先改的是遍历方式和 TLB/页释放方式，不是立即设计一套复杂的
   无锁 fork。

### 1. Linux 地址空间仍然有一把 `mmap_lock`

Linux 的 `mm_struct` 中有：

```c
struct mm_struct {
    struct maple_tree mm_mt;
    pgd_t *pgd;
    atomic_t mm_users;
    spinlock_t page_table_lock;
    struct rw_semaphore mmap_lock;
};
```

对应源码位于：

- `include/linux/mm_types.h:825`：`struct mm_struct` 开始；
- `include/linux/mm_types.h:842`：保存 VMA 的 `mm_mt`；
- `include/linux/mm_types.h:852`：顶级页表 `pgd`；
- `include/linux/mm_types.h:873`：用户引用计数 `mm_users`；
- `include/linux/mm_types.h:896`：页表和部分计数使用的锁；
- `include/linux/mm_types.h:911`：地址空间读写信号量 `mmap_lock`。

这里每个词的含义是：

- `mm`：memory management，进程内存管理对象；
- `mm_struct`：描述一个用户地址空间的结构；
- `maple_tree`：Linux 当前用于保存 VMA 区间的范围树；
- `mm_mt`：memory-management maple tree；
- `pgd`：page global directory，顶级页表；
- `mm_users`：仍在实际使用这个地址空间的用户数量；
- `page_table_lock`：保护页表或相关计数的较细粒度锁；
- `rw_semaphore`：可睡眠的读写锁；
- `mmap_lock`：保护 VMA 结构和相关地址空间变化的地址空间级锁。

因此，Linux 不是完全消除了地址空间级锁，而是把它与更细粒度的页表锁、
folio 引用计数、反向映射锁等组合使用。

### 2. Linux fork 的锁范围

Linux `kernel/fork.c:635-800` 中的 `dup_mmap()` 核心顺序是：

```c
if (mmap_write_lock_killable(oldmm))
    return -EINTR;

mmap_write_lock_nested(mm, SINGLE_DEPTH_NESTING);

retval = __mt_dup(&oldmm->mm_mt, &mm->mm_mt, GFP_KERNEL);

for_each_vma(vmi, mpnt) {
    tmp = vm_area_dup(mpnt);
    vma_iter_bulk_store(&vmi, tmp);
    if (!(tmp->vm_flags & VM_WIPEONFORK))
        retval = copy_page_range(tmp, mpnt);
}

mmap_write_unlock(mm);
flush_tlb_mm(oldmm);
mmap_write_unlock(oldmm);
```

这段代码明确说明：

- `oldmm` 是父地址空间；
- `mm` 是刚创建的子地址空间；
- 先取得父 `mmap_write_lock`；
- 再取得尚未发布的子地址空间写锁；
- VMA 和必要的 PTE 复制完成后才释放；
- 父进程因为 COW 被降级为只读的 PTE，在结尾统一刷新一次 TLB。

所以 fork 期间，同一父地址空间里的其他线程确实可能被这个写锁挡住。Linux
并没有做到“fork 与同一进程的全部缺页完全并行”。

Linux 的第一层优化是 `__mt_dup()`：

```c
/* Use __mt_dup() to efficiently build an identical maple tree. */
retval = __mt_dup(&oldmm->mm_mt, &mm->mm_mt, GFP_KERNEL);
```

它先按原树形状高效复制 Maple Tree，再用：

```c
vma_iter_bulk_store(&vmi, tmp);
```

把已经复制好的 VMA 节点批量替换进去。Moss 当前是在
`AddrSpace::try_clone()` 中遍历 `BTreeMap`，为每个 VMA 构造新 backend，
再逐个调用 `areas.map()` 插入。VMA 数很多时，Linux 的树级复制更省分配和
重新平衡开销。不过从当前源码看，Moss 更大的问题仍是 PTE 扫描，VMA 树复制
应排在它之后。

### 3. Linux 会跳过能够由缺页正确重建的 VMA

Linux `mm/memory.c:1324-1354`：

```c
static bool
vma_needs_copy(struct vm_area_struct *dst_vma,
               struct vm_area_struct *src_vma)
{
    if (userfaultfd_wp(dst_vma))
        return true;

    if (src_vma->vm_flags & (VM_PFNMAP | VM_MIXEDMAP))
        return true;

    if (src_vma->anon_vma)
        return true;

    return false;
}
```

随后 `copy_page_range()` 的第一件事是：

```c
if (!vma_needs_copy(dst_vma, src_vma))
    return 0;
```

这不是说“所有文件映射都不复制”，而是：

- userfaultfd 写保护信息无法从页缓存恢复，必须复制；
- 直接 PFN 或混合映射有特殊 PTE 语义，必须复制；
- 已经建立 `anon_vma` 的匿名/COW 映射有私有状态，必须复制；
- 其余映射可以让子进程第一次访问时重新缺页；
- 大型共享映射或尚未产生匿名 COW 页的私有只读映射因此可以省掉整段 PTE
  复制。

Moss 当前 `CowBackend::clone_map()` 只有一个粗粒度快速路径：

```rust
if self.mapped_pages.load(Ordering::Relaxed) == 0 {
    return Ok(Backend::Cow(new_backend));
}
```

只要这个 VMA 有一个已映射页，就会扫描整个 VMA。并且当前代码遇到
`file_pages` 中的干净文件缓存页时，仍然：

```rust
old_pt.protect(vaddr, cow_flags)?;
new_pt.map(vaddr, paddr, self.size, cow_flags)?;
new_backend.file_pages.insert(vaddr, page);
```

这种干净文件页在子地址空间里通常可以不安装 PTE：子进程访问时可再次从共享
文件页缓存建立只读映射。真正必须继承的是已经脱离文件缓存语义的匿名私有/COW
页。当前 `file_pages.get(vaddr)` 已经提供了区分这两类页的基础。

### 4. Linux 不按 4 KiB 虚拟地址盲扫大空洞

Linux `copy_page_range()` 从顶级页表开始：

```c
dst_pgd = pgd_offset(dst_mm, addr);
src_pgd = pgd_offset(src_mm, addr);
do {
    next = pgd_addr_end(addr, end);
    if (pgd_none_or_clear_bad(src_pgd))
        continue;
    if (copy_p4d_range(...))
        ...
} while (...);
```

下一级同样检查：

```c
if (p4d_none_or_clear_bad(src_p4d))
    continue;
```

再下一级：

```c
if (pud_none_or_clear_bad(src_pud))
    continue;
```

到 PMD：

```c
if (pmd_none_or_clear_bad(src_pmd))
    continue;
```

只有 PMD 指向实际的末级页表时，才进入 `copy_pte_range()`。

以常见三级 4 KiB 页表为例：

```text
根项为空       -> 一次跳过很大的地址范围
中间项为空     -> 一次跳过约 1 GiB 或 2 MiB 范围
末级表存在     -> 才检查其中的 PTE
```

具体覆盖大小由体系结构页表级数决定，关键不是某个固定数字，而是“空的上级
页表项可以一次跳过它覆盖的整个子树”。

Moss 当前是：

```rust
for vaddr in pages_in(range, self.size)? {
    match old_pt.query(vaddr) {
        ...
        Err(PagingError::NotMapped) => {}
    }
}
```

对 4 KiB backend 来说，`pages_in()` 每次只前进 4 KiB；`query()` 又从页表
上层走到下层。如果一个 1 GiB VMA 只驻留少量页，Moss 仍可能做 262,144 次
虚拟页检查，而 Linux 会在上级空项处直接跨过大块空洞。

`mapped_pages` 只能回答“是否为零”，不能告诉代码“具体哪些页驻留”。这就是
为什么它能优化完全空的 VMA，却不能优化稀疏 VMA。

### 5. Linux 的 PTE 内层也不是一次把锁持到底

Linux `mm/memory.c:1080-1224` 的 `copy_pte_range()`：

```c
again:
    progress = 0;
    ...
    dst_pte = pte_alloc_map_lock(...);
    src_pte = pte_offset_map_nolock(...);
    spin_lock_nested(src_ptl, SINGLE_DEPTH_NESTING);
    ...
    do {
        if (progress >= 32) {
            progress = 0;
            if (need_resched() ||
                spin_needbreak(src_ptl) ||
                spin_needbreak(dst_ptl))
                break;
        }
        ...
    } while (...);

    pte_unmap_unlock(...);
    pte_unmap_unlock(...);
    cond_resched();
    ...
    if (addr != end)
        goto again;
```

这里要区分两层锁：

- 外层 `mmap_write_lock` 仍然持有，用于保证 VMA 和 fork 语义稳定；
- 当前一张末级页表对应的 `src_ptl`、`dst_ptl` 是更细粒度的 PTE 锁；
- 内层每取得一定进度就检查是否需要调度或是否有人等待该 PTE 锁；
- 必要时释放 PTE 锁，`cond_resched()` 后从 `again` 继续；
- 这样不会让自旋锁和不可抢占区无限延长。

Linux 还会通过 `copy_present_ptes()` 尝试一次处理同一 folio 的连续 PTE，
而不是每个 PTE 都完整重复引用计数和反向映射路径。

### 6. Linux exit/exec 先脱离旧地址空间，再回收

普通退出路径 `kernel/exit.c:539-574`：

```c
mmap_read_lock(mm);
...
current->mm = NULL;
...
mmap_read_unlock(mm);
mm_update_next_owner(mm);
mmput(mm);
```

`mmput()` 在 `mm_users` 减到 0 时调用 `__mmput()`，随后调用
`exit_mmap()`。

普通 exec 路径 `fs/exec.c:962-1023` 也是：

```c
tsk->active_mm = mm;
tsk->mm = mm;
activate_mm(active_mm, mm);
...
mmput(old_mm);
```

即先把当前任务切到新地址空间，再对旧地址空间减引用并回收。

Linux 同时提供 `mmput_async()`，但这里看到的普通 exit/exec 路径调用的是
同步 `mmput()`。这意味着多个正在不同 CPU 上退出的进程可以分别回收各自的
地址空间；Linux 并没有把所有旧地址空间固定串到一个全局单 worker 上。

Moss 当前先把旧地址空间放入全局 `RECLAIM_QUEUE`，再由一个 worker：

```rust
request.aspace.lock_profiled(request.owner).clear();
```

顺序处理。因此 Moss 缩短了 exit/exec 前台路径，但把不同进程的所有回收工作
串到同一个 worker 上。这个设计不会让两个不同地址空间互相争同一把
`AddrSpaceLock`，却会：

- 让死亡地址空间排队；
- 延迟归还物理页；
- 在积压时增加内存压力；
- 让全部回收工作最多只使用一个 CPU；
- 继续触发底层全局/分片 frame 元数据和分配器路径。

不过需要准确解释旧报告：exit/exec 的 AddrSpace 已不再服务正常用户缺页时，
“它的写锁持有了很久”本身不等于“活跃 rustc 被这把锁阻塞了很久”。真正仍会
影响活任务的是回收总 CPU 时间、TLB IPI、页引用元数据锁、物理内存压力和
分配器竞争。

### 7. Linux `exit_mmap()` 的三个阶段

Linux `mm/mmap.c:1916-1980` 的主体可分为三段。

第一段，最后一个用户已经离开，使用读锁稳定 VMA 视图并清 PTE：

```c
mmap_read_lock(mm);
arch_exit_mmap(mm);
...
tlb_gather_mmu_fullmm(&tlb, mm);
unmap_vmas(&tlb, &vmi.mas, vma, 0, ULONG_MAX, ULONG_MAX, false);
mmap_read_unlock(mm);
```

第二段，取得写锁，清除 VMA 树并释放页表层级：

```c
mmap_write_lock(mm);
mt_clear_in_rcu(&mm->mm_mt);
free_pgtables(&tlb, ...);
tlb_finish_mmu(&tlb);
```

第三段，关闭并释放各 VMA：

```c
do {
    remove_vma(vma, true);
    count++;
    cond_resched();
    vma = vma_next(&vmi);
} while (...);
```

这里读锁并不是为了让普通用户线程继续使用这个 mm；注释已经明确说明
“mm 的最后一个用户已经离开”。它用于满足内核内部遍历和并发观察规则。
后面的写锁也只属于一个不可再被用户使用的 mm，因此 Linux 注释把它称为
`unreachable mmap_write_lock`。

这与 Moss 当前 `clear()` 的主要区别，不是锁名字不同，而是 Linux 已明确进入
“完整销毁”模式，页表和页释放均走专用批量路径。

### 8. Linux `mmu_gather` 为什么重要

解除一个映射时，不能安全地按下面顺序做：

```text
清 PTE
立即释放并重新分配物理页
以后再刷 TLB
```

其他 CPU 可能仍缓存旧 PTE 的 TLB 翻译，并继续访问已经被重新利用的物理页，
形成 use-after-free 或数据破坏。

Linux 因此使用：

```text
清一批 PTE
记录需要失效的范围
把待释放 folio/page table 放入 gather 批次
统一刷新 TLB
确认旧翻译不可再用
批量释放 folio 和页表
```

`tlb_gather_mmu_fullmm()` 在源码注释中明确用于：

```text
mm 已没有用户，并且正在销毁完整地址空间（exit/execve）
```

`zap_pte_range()` 还会：

- 在末级页表锁下连续检查 PTE；
- 对同一大 folio 的连续 PTE 批量清除；
- 将 folio 放入 `mmu_gather`；
- 批次空间不足或必须保证顺序时才强制刷新；
- 通过 `need_resched()` 和上层 `cond_resched()` 避免长期霸占 CPU。

### 9. Moss 当前逐页 `pt.finish()` 的真实含义

Moss `CowBackend::unmap()` 当前是：

```rust
for addr in pages_in(range, self.size)? {
    if let Ok((frame, _flags, page_size)) = pt.unmap(addr) {
        let file_page = self.file_pages.remove(addr);
        pt.finish();
        ...
        frame_ref.drop_frame(frame, self.size);
    }
}
```

`PageTable64Mut` 本来能够累计 TLB 失效：

```rust
enum ToFlush {
    None,
    Addresses(ArrayVec<..., 16>),
    Full,
}
```

最多先记录 16 个地址，超过阈值就改成一次全量刷新；guard 被 Drop 时会自动
调用 `finish()`。

但是 COW unmap 每个已映射页都显式调用一次：

```rust
pt.finish();
```

于是待刷新集合每页都被清空，自动合批能力没有发挥作用。

这里不能只删这一行。它位于 `drop_frame()` 前面，是为了保证：

```text
先让旧 TLB 翻译失效，再把该物理页交还分配器
```

直接删掉而仍立即 `drop_frame()`，会破坏这个安全顺序。正确修改必须同时增加
“待释放页批次”：

```text
unmap 多个 PTE
将 frame/FolioPageRef 放入 deferred_release
pt.finish() 一次
再逐个 drop deferred_release
```

这就是 Moss 最需要借鉴的简化版 `mmu_gather`。

## 十五、Moss 应该怎样修改

下面按收益、正确性风险和依赖关系排序。顺序很重要：先减少锁内工作，再考虑
把地址空间大锁改成更复杂的乐观并发。

### 第一优先级：缩短当前 clone 父锁的实际生命周期

当前 `clone.rs`：

```rust
let mut aspace = old_aspace.lock_profiled(AddrSpaceWriteOwner::Clone);
let aspace = aspace.try_clone()?;
{
    let mut child_aspace = aspace.lock_profiled(AddrSpaceWriteOwner::Clone);
    copy_from_kernel(&mut child_aspace)?;
    inherit_wipeonfork_ranges(...)?;
}
aspace
```

第二个 `let aspace = ...` 只是遮蔽了变量名。第一个变量持有的父写锁 guard
不会因为名字被遮蔽而立刻 Drop，而会活到外层作用域结束。因此父锁还覆盖了
只修改子地址空间的 `copy_from_kernel()` 和 `inherit_wipeonfork_ranges()`。

应改成明确作用域：

```rust
let child_aspace = {
    let mut parent = old_aspace.lock_profiled(AddrSpaceWriteOwner::CloneParent);
    parent.try_clone()?
};

{
    let mut child = child_aspace.lock_profiled(AddrSpaceWriteOwner::CloneChild);
    copy_from_kernel(&mut child)?;
    inherit_wipeonfork_ranges(..., &mut child)?;
}

child_aspace
```

这项修改：

- 不改变 fork 期间必须稳定父 VMA/PTE 的语义；
- 不声称消除父地址空间写锁；
- 只把明确属于子地址空间的工作移出父锁；
- 风险最低，应最先做。

同时应把探针从一个 `Clone` 标签拆成：

- `CloneParent`；
- `CloneChildBuild`；
- `CloneKernelMappings`；
- 可选的 `CloneCowPteWalk`。

否则父锁和子锁的嵌套区间仍会被相加，无法验证实际缩短了多少。

### 第二优先级：给页表增加“只遍历已存在映射”的层级 walker

不要再把 `mapped_pages > 0` 之后的整个 VMA 交给 `pages_in()` 逐 4 KiB
扫描。应在 `page_table_multiarch` 增加类似：

```rust
pub fn for_each_present(
    &self,
    range: VirtAddrRange,
    visit: impl FnMut(
        VirtAddr,
        PhysAddr,
        MappingFlags,
        PageSize,
    ) -> PagingResult<()>,
) -> PagingResult<()>;
```

实现原则：

1. 从根页表开始；
2. 上级 entry 不存在时，直接跳到该 entry 覆盖范围末尾；
3. 遇到 1 GiB/2 MiB huge entry 时一次返回该映射；
4. 只有末级页表存在时才检查其中的 4 KiB PTE；
5. range 起止不对齐某级覆盖范围时只裁剪边界；
6. 跳过 `copy_from` 借用的内核顶层 entry；
7. 初版即使对每个已存在 PTE 再调用一次 `protect()`，也已经从
   `O(虚拟范围页数)` 降到接近 `O(实际页表节点 + 驻留页数)`；
8. 后续再把直接修改 source PTE 和连续 PTE 批量处理融合进 walker。

clone 和 clear 都应复用这一套遍历器，不能分别维护两份容易偏离的页表扫描
逻辑。

不要先增加一棵独立的“驻留地址 BTreeMap”来替代页表 walker。独立索引会要求
每次 map、remap、unmap、fault race 和错误回滚都同步维护，页表与索引失配时
会产生比性能问题更严重的内存泄漏或错误释放。页表本身应是驻留映射的权威
来源。

### 第三优先级：fork 跳过干净文件缓存 PTE

在当前 `CowBackend::clone_map()` 中：

```rust
if let Some(page) = self.file_pages.get(vaddr) {
    ...
}
```

说明该 PTE 仍指向由文件 folio 缓存直接提供的只读页。对于这种页，子地址空间
可以：

```text
复制 VMA/backend 文件信息
不复制该 PTE
不向 child.file_pages 插入引用
子进程首次访问时从同一文件页缓存重新建立 PTE
```

而 `file_pages` 中没有记录、却在 COW backend 中驻留的普通 frame，代表匿名页
或写后私有页，必须继续共享 frame、增加引用计数并把父子 PTE 都设成只读。

实施时必须增加以下静态不变量：

- `file_pages` 中的页只能以无 WRITE PTE 发布；
- `file_page.paddr()` 必须等于当前 PTE paddr；
- 被跳过的子 PTE 不增加 `new_backend.mapped_pages`；
- 子进程 fault 能依据 `file`、`file_start`、`file_end` 重建同一文件内容；
- MAP_PRIVATE 已写脏并匿名化的页绝不能被误判为可重建文件页。

这一项比照搬 Linux 的 VMA 级 `anon_vma` 判断更适合 Moss 当前结构，因为 Moss
已经按页保存 `file_pages`，可以只跳过干净文件页，而保留同一 VMA 中已经匿名
COW 的页。

### 第四优先级：实现简化版 `mmu_gather`

先只服务 COW unmap 和死亡地址空间 clear，结构可从小版本开始：

```rust
enum DeferredPage {
    Anonymous {
        paddr: PhysAddr,
        size: PageSize,
    },
    File(FolioPageRef),
}

struct UnmapGather {
    pages: Vec<DeferredPage>,
}
```

每批流程必须是：

```text
1. 层级 walker 找到已存在 PTE；
2. 清 PTE，将 TLB 失效记录留在同一个 PageTableMut；
3. 将匿名 frame 或 FolioPageRef 放入 UnmapGather；
4. 达到批量上限、需要调度或 VMA 结束时调用 pt.finish()；
5. finish 返回后才释放 gather 中的页引用；
6. 清空 gather，必要时 cond_resched/yield，再继续。
```

初始批量上限不应拍脑袋写成固定 1 MiB 虚拟范围。应按**实际已清 PTE 数量**或
gather 容量控制，例如先用 256 个实际驻留页：

- 稀疏地址空间不会因为巨大虚拟空洞反复提交空批次；
- 256 页的引用数组规模可控；
- 每批一次 TLB 刷新，而不是每个驻留页一次；
- 后续可以根据测得的 flush 时间和队列压力调整。

这里“256”只是保守起始参数，必须在探针中记录：

- 每批实际 PTE 数；
- 每批页表层级节点数；
- 每批 `pt.finish()` 时间；
- 每批释放 frame/FolioPageRef 时间；
- 批次数和强制 full flush 次数。

在完整 `fullmm` 模式可完全省掉哪些远程 TLB 操作，要等 Moss 明确证明：

- 没有 CPU 仍运行该页表根；
- task 的旧页表引用已全部解除；
- ASID/地址空间标识不会让旧翻译命中新分配页；
- context switch 与 IPI 有所需的内存顺序。

在这些条件没有代码级证明之前，先采用“批量刷新后释放”，不要直接仿照
Linux 的 `fullmm` 优化而跳过必要 shootdown。

### 第五优先级：让死亡 AddrSpace 变成 worker 独占的回收对象

当前 worker 即使面对已经死亡的地址空间，仍取得：

```rust
AddrSpaceLock write guard
```

更合适的 Rust 结构是，在确认最后一个活用户已经离开后，短暂取得写锁并把
可销毁内容移动出来：

```rust
struct ReclaimImage {
    areas: MemorySet<Backend, VmaMetadata>,
    page_table: PageTable,
}
```

然后：

```text
活 AddrSpace -> 标记 Dead/Empty
areas 和 page_table 所有权 -> ReclaimImage
释放 AddrSpace 写锁
worker 独占 ReclaimImage 完成层级 unmap、TLB gather 和引用释放
```

这样做的收益是：

- 回收期间不再需要长期持有一个形式上的 AddrSpace 写锁；
- 所有权直接证明不会与普通 fault/mmap 并发；
- 后续可以安全地使用有限多个 reclaim worker；
- 探针能把“移交锁时间”和“实际页回收时间”分开。

它本身不会神奇地减少 PTE 数或引用计数工作，所以应在层级 walker 和 gather
之后做，不能只通过“看不到长写锁”来宣称优化成功。

### 第六优先级：再决定是否增加多个 reclaim worker

Linux 普通 `mmput()` 允许不同退出任务各自在当前上下文回收，因此天然可能
并行。Moss 可以在回收数据路径变轻后，将单 worker 改为有上限的工作队列。

不建议立即开 8 个回收 worker。BuildStorm 同时需要 8 个 CPU 做 rustc，过多
回收线程会与编译争 CPU，并放大分配器和 frame 元数据竞争。

更稳妥的策略是：

- 8 CPU 初始最多 2 个回收 worker；
- 队列为空时退出；
- pending resident pages 或 pending bytes 超过高水位时才启第二个；
- 内存水位紧张时允许前台协助回收；
- 记录 worker busy time、队列深度、pending pages 和编译吞吐；
- 以 10 分钟固定窗口和一次完整测试共同验收。

是否从 1 增到 2 必须由这些数据决定，不能仅因为 Linux 能并行回收就照搬。

### 第七优先级：将 COW frame 引用计数从 BTreeMap 移到页描述符

当前匿名 COW 页的引用路径是：

```text
paddr
-> 64 路 shard
-> SpinNoIrq<BTreeMap>
-> Arc<SpinNoIrq<FrameRefCnt>>
-> 修改 usize 引用计数
```

64 路分片已经比单个全局表好，但 clone/clear 每个匿名页仍需要：

- 计算 shard；
- 取得 BTreeMap 锁；
- `O(log n)` 查找；
- 克隆 Arc；
- 再取得该 frame 的锁；
- 引用归零时再次进入表并删除节点。

Linux 的 `struct page`/folio 元数据随物理页存在，引用计数通常可直接从 page
descriptor 原子增减，不需要先在一棵全局平衡树里按物理地址查找。

Moss 可逐步引入按 PFN 索引的页描述符。若希望每项保持 8 字节，可以把状态和
引用计数编码进同一个原子字：

```rust
struct PageDescriptor {
    state_and_refs: AtomicU64,
}
```

其中 PFN 是 `paddr / 4 KiB`。对 8 GiB、4 KiB 基页约有 2,097,152 个 PFN；
每项 8 字节的原始量级约 16 MiB。若把状态和引用计数拆成多个原子字段，受
对齐影响，实际占用还会更大。实现可只为 managed physical range
分配、按段分配，不能直接假设整个物理地址空间连续。

这项修改需要与分配器的 FREE/ALLOCATED 状态转换共同设计，保证引用从 1 减到
0 和页面进入 PCP/free list 之间没有重新分配竞态。不能只把 `usize` 换成
`AtomicUsize` 就删除现有表锁。

### 第八优先级：最后才考虑把 Linux 的父 `mmap_write_lock` 也拆掉

在完成：

- 父锁提前释放；
- 稀疏页表层级遍历；
- 干净文件页延迟 fault；
- clone TLB 统计；
- frame 引用路径优化；

之后，如果 `CloneParent` 仍是主要热点，才考虑 generation/sequence 方案：

```text
prepare：记录 VMA generation 和需要继承的 resident PTE
execute：锁外分配子页表和元数据
commit：重新取得父写锁，验证 generation，原子降级父 PTE 并安装子 PTE
stale：有限次数重试
```

它难在 COW 正确性：

- 父 PTE 的 WRITE 位降级和子 PTE 发布必须形成一致的可见顺序；
- 同时发生的父写缺页、munmap、mprotect、exec 必须被 generation 发现；
- 引用计数增加失败时必须完整回滚；
- TLB shootdown 必须在允许父任务继续写之前完成。

Linux 6.12.96 本身仍选择在 `dup_mmap()` 中持父 `mmap_write_lock`，说明先把
锁内工作降到足够小，是风险更低且经过验证的路线。

## 十六、建议的验收顺序

每一步都应先做静态不变量检查和定向单元测试，再执行昂贵的 QEMU 完整测试。

### 阶段 A：低风险锁范围修正

修改：

- 父/子 clone guard 明确分作用域；
- 拆分探针标签。

验收：

- `CloneParent` 最大持锁时间下降；
- `CloneChildBuild` 不再被计入父锁；
- fork/COW 功能测试不变。

### 阶段 B：只遍历 resident PTE

修改：

- `page_table_multiarch` 层级 walker；
- clone 和 clear 改用 walker；
- clean file PTE 在 child 中 lazy。

验收指标不能只看“编译了多少 crate”，还要记录：

```text
fork_vma_virtual_pages
fork_present_pages
fork_page_table_nodes
fork_clean_file_pages_skipped
fork_parent_lock_ns
```

关键比值是：

```text
present_pages / virtual_pages
```

越稀疏，层级 walker 的收益越大。

### 阶段 C：TLB gather

修改：

- COW unmap 延迟释放；
- 多页一次 `pt.finish()`；
- 按实际驻留页成批。

验收：

```text
unmapped_present_pages
tlb_finish_calls
pages_per_tlb_finish
deferred_release_ns
full_flush_count
```

预期不是“flush 为零”，而是：

```text
pages_per_tlb_finish 从接近 1 显著提高
```

并且不得出现 frame 重用、COW 数据串页或 stale TLB 错误。

### 阶段 D：死亡地址空间所有权和回收并发

修改：

- `ReclaimImage`；
- 根据队列水位最多启两个 worker。

验收：

- AddrSpace 移交写锁很短；
- pending pages 不再长期增长；
- 两 worker 没有降低 rustc CPU 利用率；
- 10 分钟固定窗口和完整 BuildStorm 都优于单 worker；
- 内存峰值和总 TLB IPI 没有恶化。

## 十七、当前最准确的优先级结论

若现在只允许选择三项，应按下面顺序：

1. **明确释放 clone 父 guard，并拆分父/子探针**；
2. **实现页表层级 resident walker，clone 跳过干净文件缓存 PTE**；
3. **实现带延迟释放的 unmap gather，消除 COW 每页一次 `pt.finish()`**。

原因不是抽象地“Linux 更成熟”，而是当前两边源码存在三个可直接验证的差异：

| 路径 | Moss 当前实现 | Linux 6.12.96 |
|---|---|---|
| fork 锁 | 父写锁，并因变量遮蔽延伸到子后处理 | 父写锁，但只覆盖真正的 dup_mmap |
| 稀疏映射 | 只要 count 非零就逐 4 KiB 扫整个 VMA | 按 PGD/P4D/PUD/PMD 跳过空子树 |
| 可重建文件页 | child 仍复制 PTE 和页引用 | 可由 fault 重建的 VMA 可跳过 PTE copy |
| PTE 内层 | 每页 query/protect/map 和引用表操作 | PTE 页锁、连续 folio 批处理、可调度断点 |
| clear TLB | COW 每个 resident 页显式 finish | mmu_gather 清批次、flush、再释放 |
| exit/exec 回收 | 全局单 worker 串行所有旧 AddrSpace | 普通 mmput 在不同任务上下文可并行 |

这三项完成之前，不建议先投入大量代码做无锁/乐观 fork。否则只是让
`O(巨大虚拟范围)` 的工作在更复杂的并发协议下运行，既难验证，也未必更快。

## 十八、2026-07-24 第一阶段实现状态

本节只记录已经落入当前工作树并通过定向检查的事实，不把尚未完成的整轮
BuildStorm 测量写成性能结论。

### 1. clone 父锁与子锁已经分开

`CloneArgs::build_task()` 现在用显式作用域持有 `CloneParent`：

```text
取得父 AddrSpace 写锁
-> try_clone()
-> 立即释放父锁
-> 取得子 AddrSpace 写锁
-> copy_from_kernel / wipe-on-fork
```

探针也拆成 `CloneParent` 和 `CloneChild`。因此下一次完整采样可以直接判断：

- 父地址空间真正阻塞缺页 reader 的时间；
- 只修改尚未运行的子地址空间所花的时间。

这项修改没有把父 VMA/PTE 的 COW 提交移到锁外；它只删除了变量遮蔽造成的
父锁多余延伸。

### 2. clone 和 COW clear 已改为层级 resident walker

`PageTable64::next_present_mapping()` 从页表根逐级下降：

- 空的上层 PTE 会一次跳过它覆盖的整个虚拟区间；
- 只返回真正 present 的 4 KiB、2 MiB 或 1 GiB 映射；
- 调用者按返回的页大小推进 cursor。

当前 COW fork 不再因为 VMA 中存在一个 resident page 就逐 4 KiB 扫完整个
虚拟范围。父进程中直接映射、只读且仍由文件 folio 支撑的 clean page 不复制
到子页表；子进程第一次访问时可从同一文件页缓存重新 fault。匿名页和已经
私有化的脏页仍按正常 COW 规则共享，父 PTE 会去掉 WRITE，物理页引用计数会
增加。

### 3. COW unmap 已采用“失效后释放”的 gather

每一批最多收集 256 个实际 resident PTE：

```text
清除一批 PTE
-> 一次 PageTableMut::finish()
-> 确认所有相关 CPU 的旧 TLB 已失效
-> 才减少匿名 frame 引用或释放文件 folio 引用
```

批量依据是实际找到的 PTE 数，不是虚拟地址字节数。错误返回也必须先完成
已经排队的 TLB 失效，再释放 deferred 引用，避免旧 TLB 指向已经复用的
物理页。

### 4. RISC-V 使用 SBI RFENCE 完成远程 TLB shootdown

静态审查发现，原 RISC-V `finish()` 只执行本核 `sfence.vma`，而 LoongArch
已有全核刷新回调。直接复用普通 S-mode IPI 广播后，8 核短启动立即在首个
关中断缺页上下文触发断言：

```text
synchronous IPI broadcast requires interrupts to be enabled
```

最终实现没有关闭断言，也没有在持有 `SpinNoIrq` 锁时临时打开中断，而是采用
Linux RISC-V 同样优先使用的 SBI RFENCE：

```text
本 hart: sfence.vma
其他 hart: SBI remote_sfence_vma
```

OpenSBI 在 M-mode 同步完成远程 fence，不要求目标 hart 先处理一个 S-mode
IPI，因此可以安全用于关中断的 page-fault trap。4 KiB 更新只刷新对应地址；
页表 gather 超过地址阈值后仍由现有 `PageTableMut` 合并为一次全 TLB 刷新。

### 5. 当前验证边界

已经完成：

- RISC-V release + guest-profile 构建；
- LoongArch release 构建；
- RISC-V 8 vCPU、8 GiB、`-snapshot` 的 40 秒短启动；
- cagent 10/10 通过；
- BuildStorm 预构建进入 29/446 后按计划终止；
- 没有 panic、死锁或残留 QEMU 进程。

尚未完成：

- 全新镜像的一小时级完整 BuildStorm；
- 新旧 `CloneParent`、resident PTE 数、TLB gather 次数的完整采样对比；
- 正式阶段总时间对比。

因此当前只能说三条数据路径已实现并通过短时正确性检查，不能宣称它们已经
带来确定的墙钟提速。
