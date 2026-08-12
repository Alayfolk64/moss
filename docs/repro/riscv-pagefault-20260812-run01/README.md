# RISC-V 非零 Boot HART 内核缺页排查记录

## 1. 问题与结论

评测机第一次失败时，OpenSBI 选择 Boot HART 5；第二次成功时选择 Boot
HART 0。本地使用相同的 16 GiB、8 CPU 配置又在 Boot HART 3 复现了内核
缺页。

三个结果共同指向 CPU 编号边界：Moss 将启动 hart 放到 logical CPU 0，但
RISC-V SBI 的 `HartMask` 使用 firmware-visible hart ID。原实现直接把 logical
CPU ID 放进 `HartMask`，非 0 hart 启动后可能向错误的 CPU 发送 TLB flush。

故障链条是：页表映射被撤销后，目标 hart 没有收到正确的 `SFENCE.VMA`，继续
使用过期翻译访问已经回收的物理页，最终破坏内存或页表。崩溃最后出现在 allocator
写空闲链表时，但 allocator 是发现损坏的位置，不一定是最初制造损坏的位置。

第一轮修复参考 Linux RISC-V：保留唯一的 logical CPU → hart ID 映射，
在进入 SBI 前转换；RFENCE 根据 XLEN 对 hart ID 分组。这修正了一个
确定的 CPU 编号错误，但后续五轮压测中有一轮再次崩溃，所以不能把它当作
整个内核缺页问题的完整修复。

- Linux CPU/hart 映射：<https://github.com/torvalds/linux/blob/master/arch/riscv/include/asm/smp.h>
- Linux SBI IPI/RFENCE：<https://github.com/torvalds/linux/blob/master/arch/riscv/kernel/sbi.c>
- Linux TLB flush：<https://github.com/torvalds/linux/blob/master/arch/riscv/mm/tlbflush.c>
- SBI RFENCE 规范：<https://docs.riscv.org/reference/sbi/v3.0/ext-rfence.html>

## 2. 为什么先复现

偶现问题不能只凭一次成功判断修好。先固定提交、镜像、内存、CPU 数和磁盘快照，
可以区分以下情况：

- 问题真的消失；
- 只是 Boot HART 恰好变成 0；
- 修改只降低并发窗口，没有修正错误状态；
- 运行的镜像和用于反汇编的 ELF 不一致。

本目录保存了基线运行：

```text
run01.log          本地失败的原始串口日志
kernel-rv          本次运行的 QEMU 包装镜像
starryos.sym.elf   与镜像 payload 对应的符号 ELF
```

查看文件哈希：

```bash
cd /home/wuxun/Projects/moss-main
sha256sum docs/repro/riscv-pagefault-20260812-run01/kernel-rv
sha256sum docs/repro/riscv-pagefault-20260812-run01/starryos.sym.elf
```

`sha256sum` 回答“现在分析的文件是否仍是当时运行的文件”。基线哈希为：

```text
2a0317c97f683fe780e614a5d1674370d281add76bc4938a7e0196e2607b03b1  kernel-rv
8b9396709b8ab75a3da898ba144389b5c8eb674c2af8b979c06bca426557544c  starryos.sym.elf
```

## 3. 先比较失败和成功日志

提取启动 hart、测试结束标志和内核异常：

```bash
rg -n 'Boot HART ID|END buildstorm|Unhandled Supervisor Page Fault' '/home/wuxun/Desktop/Riscv输出 (21).txt'
rg -n 'Boot HART ID|END buildstorm|Unhandled Supervisor Page Fault' '/home/wuxun/Downloads/Riscv输出 (23).txt'
rg -n 'Boot HART ID|END buildstorm|Unhandled Supervisor Page Fault' docs/repro/riscv-pagefault-20260812-run01/run01.log
```

`rg -n` 同时打印行号，便于回到日志上下文。应看到：

| 运行 | Boot HART | 结果 |
| --- | ---: | --- |
| 评测失败 | 5 | allocator 中 Supervisor Page Fault |
| 评测成功 | 0 | buildstorm 完成 |
| 本地复现 | 3 | allocator 中 Supervisor Page Fault |

这是一条强相关证据，但还不是单独的因果证明，所以还要继续做反汇编和源码审计。

## 4. 确认应该反汇编哪个文件

```bash
file kernel-rv
file target/riscv64gc-unknown-none-elf/release/starryos
```

本项目的 `kernel-rv` 是把裸二进制包装进 ELF 的 QEMU 启动文件，通常没有原内核
函数符号。`target/.../starryos` 或使用 `DWARF=y` 生成的 `kernel-rv.dbg.elf`
才适合把 PC 还原为 Rust 函数。

重新生成带 DWARF 的镜像：

```bash
make DWARF=y kernel-rv
```

`DWARF=y` 保留 `kernel-rv.dbg.elf`，不会改变 GDB 的使用方式。

## 5. 用 GDB 定位故障指令

进入 GDB：

```bash
gdb-multiarch docs/repro/riscv-pagefault-20260812-run01/starryos.sym.elf
```

在 GDB 中逐条输入：

```gdb
info symbol 0xffffffc0803f4f24
x/8i 0xffffffc0803f4f18
quit
```

- `info symbol` 回答“PC 属于哪个函数”。
- `x/8i` 从指定地址开始显示 8 条指令；`i` 表示 instruction。
- 本地复现的 PC 是 `ax_slab_allocator::Heap::deallocate + 280`。
- 故障指令是 `sd a2,0(a1)`，即向 `a1` 指向的已释放对象写空闲链表指针。

评测失败的 PC 也可用同样方法查询：

```gdb
info symbol 0xffffffc0803f4ef2
x/8i 0xffffffc0803f4ee8
```

两个 PC 都在 allocator 的 deallocate 路径，但分别经过 slab 和 buddy 大小级别，
说明它不像某一种对象的固定越界，更像底层映射或物理页生命周期被破坏。

## 6. 浏览完整反汇编

```bash
rust-objdump -d -C docs/repro/riscv-pagefault-20260812-run01/starryos.sym.elf | less
```

- `-d` 反汇编可执行代码。
- `-C` 将 Rust 符号还原为可读函数名。
- 在 `less` 中输入 `/Heap::deallocate` 搜索，按 `n` 查看下一处，按 `q` 退出。

如果只关心一小段地址：

```bash
rust-objdump -d -C --start-address=0xffffffc0803f4ee0 --stop-address=0xffffffc0803f4f40 docs/repro/riscv-pagefault-20260812-run01/starryos.sym.elf
```

长参数版本适合保存证据；第一次学习时优先使用前面的 `gdb-multiarch` 交互方式。

## 7. 从症状回到源码机制

先找所有 CPU ID 进入 SBI 或 PLIC 的位置：

```bash
rg -n 'HartMask|remote_sfence|this_context|cpu_hardware_id' kernel vendor
```

再看 logical/hardware 映射如何建立：

```bash
sed -n '60,130p' vendor/axhal/src/dtb.rs
sed -n '120,180p' vendor/axhal/src/lib.rs
sed -n '1,70p' vendor/axruntime/src/mp.rs
```

这里要回答四个问题：

1. DT 中的 `reg` 是 hardware hart ID。
2. Moss 为什么把实际 Boot HART 交换到 logical slot 0。
3. 调度器和 per-CPU 数组为什么必须继续使用 dense logical CPU ID。
4. 哪些 SBI/PLIC 接口明确要求 hardware ID。

查看引入相关行为的提交：

```bash
git log --oneline -- kernel/src/mm/tlb.rs vendor/axhal/src/dtb.rs
git show 9428f1df -- kernel/src/mm/tlb.rs
```

`git log` 用来找历史提交，`git show` 用来核对当时的设计假设，而不是直接回退整个
优化提交。

## 8. 为什么第一轮没有用两个简单补丁

### 把回收 worker 限制为 1

它不会改变发错 hart 的 `HartMask`，所以不能修复第一轮已确认的 CPU ID
错误。第 5 轮再次崩溃后，“回收 worker 数是否改变复现概率”变成了有价值的
诊断 A/B；但即使限制为 1 后多轮成功，也只能证明和并发回收相关，不等于页表
生命周期已被正确修复。

### 只修改 TLB 的一个位移

可以覆盖当时的 TLB 崩溃，但 SBI IPI 和 QEMU PLIC context 仍存在相同的
logical/hardware ID 混用。第一轮因此采用小型边界重构：HAL 上层使用
logical CPU，进入硬件接口时统一转换；没有重构地址空间、COW 或 allocator。

## 9. 构建和 A/B 验证

检查补丁格式和三个目标：

```bash
git diff --check
make kernel-rv
make kernel-vf2
make kernel-la
```

运行评测配置：

```bash
./scripts/run_rv.sh
```

查看验收标志：

```bash
rg -n 'Boot HART ID|MOSS_SMP online|END buildstorm|Unhandled Supervisor Page Fault' docs/output_rv.txt
```

Boot HART 0 的成功只能证明兼容性。必须至少保存两次非 0 Boot HART 的完整成功
日志，并同时满足：8 CPU online、cagent 完成、buildstorm 完成、没有 page fault、
panic 或异常重启。

## 10. 第一轮修复候选的实际结果

候选镜像和符号文件保存在：

```text
docs/repro/riscv-pagefault-20260812-fixed/kernel-rv
docs/repro/riscv-pagefault-20260812-fixed/starryos.sym.elf
```

SHA256：

```text
70fb744b501ff6d4668fce784e4492e069f64a06fe6ba29b3f38487eed858782  kernel-rv
1ecaf2e24a49aacb220662b80ce56d0dd688729108d426353467776c3a5a8b64  starryos.sym.elf
```

在同一镜像、16 GiB、8 CPU、`-snapshot` 条件下完成了五轮非 0 Boot
HART 验证：

| 日志 | Boot HART | CPU online | cagent | buildstorm | 内核异常 |
| --- | ---: | ---: | --- | --- | --- |
| `run01-hart3.log` | 3 | 8 | 完成 | 493.28 s，成功 | 无 |
| `run02-hart7.log` | 7 | 8 | 完成 | 484.24 s，成功 | 无 |
| `run03-hart7.log` | 7 | 8 | 完成 | 454.53 s，成功 | 无 |
| `run04-hart5.log` | 5 | 8 | 完成 | 467.26 s，成功 | 无 |
| `run05-hart6-fail.log` | 6 | 8 | 完成 | 编译中崩溃 | Supervisor Page Fault |

用最少命令复核保存的证据：

```bash
cd /home/wuxun/Projects/moss-main
rg -n 'Boot HART ID|MOSS_SMP online|END cagent|BUILDSTORM_COMPILE|END buildstorm|Page Fault|panicked' docs/repro/riscv-pagefault-20260812-fixed/*.log
sha256sum docs/repro/riscv-pagefault-20260812-fixed/kernel-rv
```

第 5 轮的镜像 SHA256 仍是 `70fb744b...8782782`，与前四轮完全相同。
因此本组结果是 4/5 成功、1/5 失败，问题仍然可复现。Boot HART 3 和原评测
失败的 Boot HART 5 都曾成功，而 Hart 6 最终失败；这说明非 0 Boot HART
是暴露竞态的条件，不是“某个 hart 必然损坏”。

## 11. 第 5 轮新崩溃的反汇编

第 5 轮的 PC 为 `0xffffffc0803d7840`，使用本轮保存的符号 ELF：

```bash
gdb-multiarch docs/repro/riscv-pagefault-20260812-fixed/run05-starryos.sym.elf
```

在 GDB 中：

```gdb
info symbol 0xffffffc0803d7840
x/8i 0xffffffc0803d783c
quit
```

实际定位到 `axtask::run_queue::AxRunQueue::resched + 50`，故障指令为：

```asm
ld a1, 648(a2)
```

日志中 `a2 = 0xffffffc0d1d28c10`，因此本次访问地址是
`0xffffffc0d1d28e98`。它位于内核 direct-map 范围，正常情况应始终映射。
这与前两类 allocator 崩溃的共同点是“内核永久映射范围突然缺页”，
但崩溃函数不同，所以 allocator 和 scheduler 都更像受害者。

这一步只能说明 SATP 或页表内容异常，还不能区分地址空间回收、页表破坏和启动
页表竞态。下一节用启动阶段的 root 记录把三者区分开。

## 12. 根因：从核用启动页表覆盖了全局 kernel root

RISC-V 的 `TaskContext::new()` 会调用 `read_kernel_page_table()`，把全局记录的
kernel root 保存到新任务中。QEMU 平台的主核和每个从核却都通过
`write_kernel_page_table(BOOT_PT_SV39)` 启用临时启动页表；后者除了修改本核
`satp`，还会写同一个全局 `KERNEL_PAGE_TABLE_ROOT`。

因此启动 7 个从核时存在下面的跨核竞态：

1. 从核 A 把全局 root 改成 `BOOT_PT_SV39`；
2. 从核 B 创建本核的 `idle` 或 `gc` 任务，把这个临时 root 保存到
   `TaskContext.satp`；
3. 从核 A/B 随后安装正式 kernel root，但错误值已经留在任务上下文；
4. 该任务以后从用户任务切回来时，`switch_to()` 把临时 root 重新写入硬件
   `satp`；
5. 启动页表能执行 `P3[0x102]` 中的内核代码，却没有 direct-map 的
   `P3[0x103]`，所以 allocator 或 scheduler 首次访问这一段内存时缺页。

诊断镜像只在最早创建的 32 个 `TaskContext` 中记录 `saved_root` 和硬件当前
`satp`，没有改变页表值。短启动的第 6 轮抓到了：

```text
ordinal=5 saved_root=PA:0x80459000 current_root=PA:0x846ac000
ordinal=6 saved_root=PA:0x80459000 current_root=PA:0x846ac000
```

对应证据保存在：

```text
docs/repro/riscv-pagefault-20260812-fixed/run12-root-race-diag.log
docs/repro/riscv-pagefault-20260812-fixed/run12-root-race-kernel-rv
docs/repro/riscv-pagefault-20260812-fixed/run12-root-race-starryos.sym.elf
```

用最少命令复核：

```bash
rg -n 'saved_root=PA:0x80459000' docs/repro/riscv-pagefault-20260812-fixed/run12-root-race-diag.log
rust-nm -n docs/repro/riscv-pagefault-20260812-fixed/run12-root-race-starryos.sym.elf | rg BOOT_PT_SV39
```

符号 ELF 给出的 `BOOT_PT_SV39` 虚拟地址是 `0xffffffc080459000`；减去
direct-map 偏移 `0xffffffc000000000`，物理地址正是日志中的
`0x80459000`。这不是相似地址，而是同一张启动页表。

`git blame` 还能把回归边界定位到提交 `9428f1df`：该提交引入全局
`KERNEL_PAGE_TABLE_ROOT`，并让 `write_kernel_page_table()` 每次都更新它；平台
原有的从核 `init_mmu()` 随即从“只写本核 SATP”变成了“同时覆盖全局正式
root”。该提交之前不存在这条跨核发布路径。

三次故障地址分别是 `0xffffffc0d2500000`、`0xffffffc0d4ad6580` 和
`0xffffffc0d1d28e98`，Sv39 顶级索引全是 `0x103`；三次故障 PC 的顶级索引
则是 `0x102`。源码中 `BOOT_PT_SV39` 设置了 `0x102`，没有设置 `0x103`。
这解释了内核为什么可以继续取指、进入 panic 并打印寄存器，却在访问普通内核
堆对象时缺页。

诊断打印随后已经撤销，`vendor/axcpu/src/riscv/context.rs` 恢复为原 SHA256：

```text
8bdefbfded39dc1c946b63f37fbed901066ccfae11a756150f3a6554adba853f
```

## 13. 与 Linux 的实现对照及修复边界

Linux RISC-V 明确区分临时 `trampoline_pg_dir`、早期 `early_pg_dir` 和最终
`swapper_pg_dir`。从核 `secondary_start_sbi` 安装最终的
`swapper_pg_dir`，内核线程共享 `init_mm`；临时启动 root 不会被发布成所有任务
继承的正式 kernel root。

Moss 应按同样的职责边界修复，而不是给 allocator、scheduler 或地址空间回收器
打补丁：

1. 增加只安装早期 SATP、但不修改全局正式 root 的 boot 专用接口；
2. 主核和从核的 `BOOT_PT_SV39` 只走 boot 接口；
3. 只有 `axmm::init_memory_management()` 发布最终 kernel root；
4. 从核完成高地址跳转后尽早切到最终 root，再进入调度器；
5. 加启动断言，禁止 `idle/gc` 的上下文保存 `BOOT_PT_SV39`。

这属于小范围语义重构：改动点不多，但必须拆开“本核临时 SATP”和“全局正式
kernel root”两个概念。仅在 `TaskContext::new()` 发现坏值时改回正确值只能掩盖
发布错误，不能作为最终修复。

## 14. Linux 式页表分层修复结果

最终实现将“early boot 本核安装”“正式 root 单次发布”和“正式 root 本核安装”
拆成三个独立接口。QEMU 与 VisionFive2 的 `BOOT_PT_SV39` 不再修改全局正式 root，
主核内存管理使用 `compare_exchange` 发布一次正式 root，从核只读取并安装它。

验证使用同一个 SHA256 为 `8d9a67a7...e38846` 的镜像完成：

- 20/20 次 16 GiB、8 CPU 短启动成功；
- 两个独立根文件系统、两个 16 GiB/8 CPU QEMU 并发跑完全程；
- 两边均完成 cagent 和 buildstorm，buildstorm 分别为 540.86 s、546.73 s；
- 两边均无 page fault、panic、重复发布或错误 root 安装断言。

实现细节、命令、镜像和原始日志保存在：

```text
docs/repro/riscv-pagefault-20260812-linux-pt-fix/
```
