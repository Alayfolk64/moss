# pending-reschedule 复用 `self` 失败记录

## 结果

本轮没有产生可用火焰图，也不能作为性能数据。三遍静态审查和内核构建通过
后，使用与前两轮相同 SHA-256 的官方全新镜像启动；cagent 10/10 通过，
tg-xtask 预构建进行到 `8/446` 时内核 panic。

```text
Unhandled Supervisor Page Fault
sepc=0xffffffc080277a54
fault_vaddr=0xffffffc0c1ba8000
access=READ
```

对本轮 `kernel-rv.profile.elf` 反汇编后，`sepc` 对应：

```text
<PageTable64 as Drop>::drop + 0x9e
    ld a0, 0(s1)
```

也就是地址空间销毁时遍历根页表项，根页表的直接映射地址已经不可读。QEMU
随后退出，GDB 无法取得一致的 profile shards；五次导出均失败。因此：

- 不生成伪造或不完整的 SVG；
- 不把启动后约 15 秒的 CPU 数据与完整流程比较；
- 不保留本轮 pending-self 语义改动。

## 处理

本轮相对已完整通过的 acquire-fastpath 基线增加了两项组合变化：

1. preempt hook 直接解引用当前任务，不构造 `CurrentTask`；
2. `current_check_preempt_pending` 持有原 `&self` 跨过 run-queue 获取。

第一项已单独跑过完整流程但没有稳定性能收益；第二项加入后首次完整运行即
在并发地址空间销毁中 panic。即使静态上可以论证任务由调度器 `Arc` 保活，
运行结果也不足以接受这个生命周期边界。两项一起撤销，回到已有完整成功
证据的 acquire-fastpath 版本。

下一轮只给跨 crate 的短 guard 方法增加内联提示，不改变任务引用、preempt
计数、调度或页表生命周期。
