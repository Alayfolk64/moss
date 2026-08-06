# COW 物理页直接引用表实验报告

## 结论

本实验将匿名 COW frame 引用计数从 64 个分片的
`BTreeMap<PhysAddr, Arc<SpinNoIrq<FrameRefCnt>>>` 改为依据设备树物理内存
范围建立的紧凑直接索引表。正确性和双架构多核初始化均通过，但 RISC-V
两轮平均比稳定基线慢 1.88%，因此实验已回退，不能进入产品源码。

Git 节点：

```text
8e217bf7 perf: 用物理页直接索引COW引用计数
b1ccbd58 revert: 回退COW物理页直接索引实验
```

## 修改边界

实验只修改匿名 frame 引用表及其启动初始化：

- 根据 `axhal::mem::phys_ram_ranges()` 建立紧凑 PFN 索引，排除物理内存段
  之间的空洞；
- 将连续逻辑页交错放入 64 个 `SpinNoPreempt<Vec<usize>>` 分片；
- 以直接槽位访问替代每次 `BTreeMap` 查找、`Arc` clone 和逐 frame 锁；
- 在创建第一个用户地址空间前初始化引用表；
- 保留原有 file-page 引用表、clone 页表遍历和页表操作实现，不混入下一组
  优化。

8 GiB RAM 对应 2,097,152 个 4 KiB 页，`usize` 引用槽约占 16 MiB。该内存
在启动阶段一次分配，不计入 BuildStorm 的 `BUILDSTORM_BEGIN` 到
`BUILDSTORM_COMPILE` 计时区间，但会持续占用客体内存和缓存工作集。

## 评测口径

```sh
# 关闭 DWARF、MEMTRACK 和 guest profiler
./scripts/run_rv.sh
./scripts/run_la.sh
```

两种架构串行运行，均使用当前评测镜像、8 GiB、`-snapshot`。RISC-V 为
8 vCPU，LoongArch 为 12 vCPU。`pidstat` 只在宿主机采样 QEMU 进程，不
修改客体内核。

| 架构 | 稳定基线 | 实验结果 | 实验均值 | 相对基线 | QEMU 峰值 |
| --- | ---: | ---: | ---: | ---: | ---: |
| RISC-V | 483.23 s | 494.66 / 489.98 s | 492.32 s | +9.09 s / +1.88% | 802% |
| LoongArch | 378.69 s | 376.94 s | 376.94 s | -1.75 s / -0.46% | 1189% |

RISC-V 两轮和 LoongArch 一轮均输出：

```text
MOSS_DTB cpu_nodes=...
MOSS_SMP online=...
#### OS COMP TEST GROUP END cagent-glibc ####
BUILDSTORM_COMPILE mode=multi ok=true ...
#### OS COMP TEST GROUP END buildstorm-glibc ####
```

LoongArch 启动日志额外确认 `memory_regions=2`，直接索引能正确覆盖由 DT
给出的两个物理内存段。LoongArch 的 0.46% 单轮改善低于 1% 判断线，不能
当作可靠正收益；公共实现同时在 RISC-V 上稳定回退 1.88%，所以整体判负。

## 原始证据

- [RISC-V 第 1 轮控制台](rv-run1-console.log)
- [RISC-V 第 2 轮控制台](rv-run2-console.log)
- [LoongArch 第 1 轮控制台](la-run1-console.log)
- [RISC-V 第 1 轮 pidstat](rv-run1-pidstat.log)
- [RISC-V 第 2 轮 pidstat](rv-run2-pidstat.log)
- [LoongArch 第 1 轮 pidstat](la-run1-pidstat.log)

归档日志 SHA-256：

```text
9ba3dc12f261a6da4c6f778baa9bf160cf432bd1bfd690b6a91b3d3c341c640d  rv-run1-console.log
f3c4309a41b611a75a6c0c933dc3374293e1fc47d8bf2a790f1e7d4f62d82f08  rv-run2-console.log
f680ca0796d7b077cb24d9d8a47c42e96cc74f90003da0f5dbe9e8d615405514  la-run1-console.log
548d4cdb01943b72ace4653e9b22998eca5cc1956ac494668237a546dd0eebc7  rv-run1-pidstat.log
45b89e501a594451957e5fbba053aa8f4eb64643637a847fbd92a5203e107d5c  rv-run2-pidstat.log
ac0b61e709329fc060a54732bc9e0807ab08ab5ee907c66b8f2248d493a1111a  la-run1-pidstat.log
```

## 判断与下一步

实测只能证明直接引用表单独迁移没有净收益。一个合理但尚未单独验证的解释
是：当前基线的引用表只为实际匿名页稀疏分配，而直接表扩大了常驻工作集；
优化仓库中的收益可能依赖 clone/unmap 遍历、file-page 所有权和页表原语等
后续改动共同出现，不能归因到直接表本身。

下一项应继续保持最小边界：先让匿名 `CowBackend` 不再创建空的 64 分片
file-page 所有权表；确认收益后，再单独测试 file-page 分片的单元素内联。
不应把直接引用表重新混入下一次实验。
