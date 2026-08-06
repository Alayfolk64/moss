# RISC-V 回退基线外部 GDB 火焰图报告

## 结论

每核页缓存实验已通过 `65430442 revert: 回退每核页分配缓存性能实验`
完整回退。`65430442` 的源码树与实验前基线 `a2f3320d` 一致，失败实验和
回退节点均保留在 Git 历史中。

回退后以带 DWARF 和 frame pointer、但不含 guest profiler 探针的内核运行
完整 RISC-V cagent 与 BuildStorm，并通过 QEMU GDB stub 对 8 个 vCPU 做
外部栈采样。cagent 和 BuildStorm 均通过，输出：

```text
MOSS_DTB cpu_nodes=8 memory_regions=1 memory_bytes=0x200000000
MOSS_SMP online=8
#### OS COMP TEST GROUP END cagent-glibc ####
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=496.08 cores=8
#### OS COMP TEST GROUP END buildstorm-glibc ####
```

外部 GDB 会短暂停住全部 vCPU，因此 `496.08 s` 不是正式性能数据，不能与
无采样基线 `483.23 s` 比较。本轮只使用调用栈分布定位热点。

## 采样口径

| 项目 | 值 |
| --- | ---: |
| QEMU | RISC-V virt，8 GiB，8 vCPU，`-snapshot` |
| 内核 | release，DWARF，frame pointer，无 guest profiler |
| 采样尝试 | 535 |
| GDB 成功 / 失败 | 355 / 180 |
| 有效 vCPU 栈 | 2,840 |
| 活跃 / idle | 1,764 / 1,076 |
| 可完整解析的活跃内核栈 | 99 |
| `kernel-rv.dbg.elf` SHA-256 | `282532bd1b7215706f82d56e0823cdb0045b38ae5cf62ee65cd3f866bd3104a5` |

失败集中发生在 GDB 尝试从 guest 用户地址反向展开时。采样脚本随后已增加
失败后重新连接并 `detach` 的恢复逻辑，避免 GDB 异常退出后让 QEMU 保持
暂停；默认间隔也从 0.75 秒调低为每 2 秒一次，减少 stop-the-world 开销。

## 复现命令

先构建带调试信息的 release 内核，并在终端 1 启动 QEMU：

```sh
make PROFILE=n MEMTRACK=n DWARF=y ARCH=riscv64 MEM=8G SMP=8 kernel-rv
qemu-system-riscv64 -machine virt -kernel kernel-rv -m 8G -nographic \
  -smp 8 -bios default -snapshot \
  -drive file=sdcard-rv.img,if=none,format=raw,id=x0 \
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
  -no-reboot -device virtio-net-device,netdev=net \
  -netdev user,id=net -rtc base=utc -gdb tcp:127.0.0.1:1235 \
  2>&1 | tee /tmp/moss-rollback-profile-rv.log
```

终端 2 在 BuildStorm 开始后自动采样。`--flamegraph` 指向 Brendan Gregg
FlameGraph 仓库中的 `flamegraph.pl`：

```sh
python3 scripts/sample_qemu_guest_stacks.py \
  --elf kernel-rv.dbg.elf \
  --console /tmp/moss-rollback-profile-rv.log \
  --out /tmp/moss-rv-gdb-profile \
  --gdb-port 1235 \
  --flamegraph /path/to/FlameGraph/flamegraph.pl
```

本报告归档的是首次采样的原始口径：`--interval 0.75 --depth 24`。脚本现在
默认采用 `--interval 2 --depth 16`，以降低对客体进度的影响。正式耗时基线
必须去掉 `-gdb` 和采样进程后另跑。

## 火焰图

- [活跃 vCPU 火焰图](cpu-active.svg)
- [全部 vCPU 火焰图](cpu-all.svg)
- [活跃 vCPU folded stacks](cpu-active.folded)
- [全部 vCPU folded stacks](cpu-all.folded)
- [原始统计摘要](summary.json)
- [完整控制台日志](console.log)

活跃样本中有 1,449 个停在 guest 用户地址，216 个停在 trap 入口，外部
GDB 无法用内核 ELF 继续解析它们。下面只对剩余 99 个完整内核栈做
inclusive 统计；同一条栈可同时计入父函数与子函数，各行不能相加。

| 内核路径 | inclusive 样本 | 占 99 个已解析内核栈 |
| --- | ---: | ---: |
| `CloneArgs::do_clone` / `sys_clone` | 38 | 38.4% |
| `AddrSpace::try_clone -> CowBackend::clone_map` | 37 | 37.4% |
| `CowBackend::share_existing_at` | 28 | 28.3% |
| `CowBackend::unmap_present_into` | 25 | 25.3% |
| `AddrSpace::clear` / 地址空间回收 worker | 22 | 22.2% |
| 页表 `query` | 11 | 11.1% |
| 页表 `map` | 11 | 11.1% |
| frame 引用释放 / `UnmapGather::finish` | 11 | 11.1% |
| ext4 `crc32` | 5 | 5.1% |

## 判断与下一步

这轮火焰图不支持继续优先迁移每核页缓存。当前可解析的内核样本主要集中在
fork/clone 时的 COW 页表复制、匿名 frame 引用查找，以及退出时的逐页
unmap/引用释放。

下一组应从 COW 路径拆分验证，顺序建议为：

1. 将匿名 frame 引用计数从分片 `BTreeMap<PhysAddr, Arc<...>>` 改成按物理页
   直接索引的分片计数表，消除 clone/unmap 的树查找和每页 `Arc`；
2. 将文件页引用分片的常见单元素情况内联，只有冲突时才进入 `BTreeMap`；
3. 单独评测页表 `query/map/protect` 内联，不能与前两项混成一个提交；
4. 每一步先跑 RV，再跑 LA，端到端没有正收益时继续保留回退节点。

本轮外部采样无法统计锁等待和 off-CPU 时间，也不能解析用户态热点；这些是
该方法的明确边界。若下一组 COW 修改的端点变化小于约 1%，应再使用内部
guest profiler 做锁等待和逐阶段验证，而不能只依赖这 99 个内核栈。
