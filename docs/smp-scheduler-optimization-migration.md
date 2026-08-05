# 多核调度与构建性能优化迁移记录

## 1. 目标与原则

本轮迁移没有直接合并 `smp-scheduler-optimization`。该分支同时包含正确性修复、性能试验、探测代码和大量报告产物，难以解释单项收益，也不适合作为后续报告的开发历史。

迁移采用以下原则：

1. 先固定评测脚本和镜像使用方式；
2. 再从设备树读取 CPU 与内存拓扑，完成多核正确初始化；
3. 先修复多核、内存、进程和用户态兼容性问题；
4. 每组性能修改使用独立提交，保留失败试验和回退记录；
5. RISC-V 与 LoongArch 分开运行，QEMU 使用 `-snapshot`；
6. 正式性能数据使用普通 release 内核，不启用 DWARF、memtrack、guest-profile 或客体探针。

## 2. 分支与基线

| 阶段 | 分支或标签 | 结果 |
| --- | --- | --- |
| 迁移前 | `baseline/pre-smp-migration-a600574e` | 保存原始 main 基线 |
| 评测入口 | `port/00-eval-baseline` | 增加双架构决赛评测脚本，镜像写入使用快照 |
| DTB 拓扑 | `port/10-dtb-topology` | 从设备树读取 CPU 和内存，RISC-V 8 核、LA 12 核全部上线 |
| 多核 TLB | `port/20-smp-correctness` | 补齐远程 TLB 刷新 |
| 用户态正确性 | `baseline/userspace-correctness-ce552c4e` | 两架构 cagent 与 buildstorm 完整通过 |
| 调度与 rseq | `baseline/scheduler-rseq-1016s-16668037` | RISC-V buildstorm 1016.33 秒 |
| folio 页缓存 | `baseline/folio-direct-map-495s-45543403` | 首次 495.90 秒，但复测暴露低概率阻塞 |
| 稳定无探测基线 | `baseline/stable-noprobe-rv483-la379-9e8a1517` | 双架构各连续两轮通过 |

## 3. 迁移后的优化组成

### 3.1 缺页与地址空间并发

- `dc54e032 perf: 将私有缺页拆分为并发三阶段`
- `ffe367b6` 至 `7aacce41`：修复 COW 写权限、文件缺页、页缓存淘汰和远程 TLB 的并发闭环；
- `a403b560 perf: 按受影响 VMA 更新内存权限`：避免对无关 VMA 重复改权限；
- `fd862797`、`445cc8d4`：将地址空间回收移出退出关键路径并合并跨 VMA 刷新；
- `45543403 perf: 引入 folio 页缓存与只读文件页直映`：分片多页缓存、预读、事件唤醒、只读私有映射直映和稀疏页表遍历。

### 3.2 调度、IPI 与用户态并行运行时

- `61effc50 perf: 改进多核运行队列负载均衡`：在安全的用户返回边界迁移任务；
- `0a6ca5f7 fix: 清除 RISC-V IPI 的 SSIP 待决位`：避免软件中断持续待决；
- `16668037 feat: 实现 rseq 线程注册`：支持用户态运行时观察 CPU 迁移；
- `c31f872f`、`d4675f9c`、`bda61ad3`：将 nanosleep、文件锁和 POSIX 定时器从轮询改为事件驱动等待。

### 3.3 futex 与管道稳定性

- `692b85dd fix: 原子化 futex 唤醒与重排`：同时锁住源、目标等待队列，消除重排窗口内的丢唤醒；
- `09c815d0 fix: 防止并发回收 futex 等待条目`：在 futex 表锁内重新确认引用和队列状态；
- `4885d255 perf: 减少多核 futex 唤醒后的无效让出`：SMP 下不再让当前 CPU 为另一运行队列的任务无效 yield；
- `ca3ed20f fix: 退出与执行时分离共享文件描述符表`：进程退出或 exec 时只关闭自己的 FD 表副本；
- `9e8a1517 fix: 按端点方向跟踪管道关闭状态`：分别统计读端和写端，正确产生 EOF、HUP、ERR 和唤醒。

## 4. 两次偶发阻塞的定位过程

folio 首次评测得到 495.90 秒，但无探测复跑在高并发阶段后降到 3% 至 5%。现场显示八个 hart 全部进入 idle、所有运行队列为空、存活任务全部为 Blocked，因此问题不是 QEMU 未启用多核，也不是调度器遗漏 Ready 任务。

第一处现场中，最后一个 Cargo 任务停在 `FUTEX_WAIT_PRIVATE`。代码检查确认两处竞态：

1. `FUTEX_REQUEUE` 先从源队列取出，再放入目标队列，目标 futex 的并发唤醒可能落在中间窗口；
2. futex 空条目在表锁外判断后删除，可能把刚被新等待者取得的条目从表中移除。

修复后成功样本稳定在约 488 秒，但仍捕获到一次发生在编译结束后的阻塞。与运行内核逐字节校验一致的 release ELF 现场显示：Cargo 和 rustc 均已退出，只剩 `tg-xtask` 与 `tee` 在管道读取中等待 EOF。原实现发现 FD 表仍被活进程共享时会跳过退出清理，僵尸子进程继续持有管道写端，而父进程需要先读到 EOF 才调用 `wait`，构成闭环。`ca3ed20f` 与 `9e8a1517` 修复了这条退出路径。

## 5. 正式评测结果

正式内核的构建特征如下：

```text
RISC-V:   axfeat/defplat axfeat/bus-mmio axfeat/smp qemu
LoongArch: axfeat/defplat axfeat/smp qemu
```

测试使用当前评测镜像，RISC-V 和 LoongArch 完全串行运行。`pidstat` 仅在宿主机外部采样 QEMU 进程，不进入客体内核。

| 架构 | CPU 来源 | 第 1 轮 | 第 2 轮 | 均值 | QEMU 峰值 |
| --- | --- | ---: | ---: | ---: | ---: |
| RISC-V | DTB 8 核，`online=8` | 480.78 s | 485.68 s | 483.23 s | 801% |
| LoongArch | DTB 12 核，`online=12` | 379.60 s | 377.78 s | 378.69 s | 1189% |

两种架构的两轮测试均完整结束 cagent 和 buildstorm，最终输出 `BUILDSTORM_COMPILE mode=multi ok=true` 并由 QEMU 正常关机。

- RISC-V 相比 1016.33 秒的调度阶段基线减少 533.10 秒，提升 52.45%；
- RISC-V 相比 1524.47 秒的初始正确性基线减少 1041.24 秒，提升 68.30%；
- LoongArch 相比 926.08 秒的初始正确性基线减少 547.39 秒，提升 59.11%；
- LoongArch 当前均值比 RISC-V 少 104.54 秒，即快 21.63%。

## 6. 复现方式

```bash
# 两种架构必须分开运行
./scripts/run_rv.sh
./scripts/run_la.sh
```

验收日志至少应包含：

```text
MOSS_DTB cpu_nodes=...
MOSS_SMP online=...
#### OS COMP TEST GROUP END cagent-glibc ####
BUILDSTORM_COMPILE mode=multi ok=true ...
#### OS COMP TEST GROUP END buildstorm-glibc ####
```

`smp-scheduler-optimization` 中未迁移的探测功能、临时手册、火焰图原始产物和无法单独证明收益的试验代码不进入 main。后续优化应从当前稳定标签继续，每一项单独提交、单独评测。
