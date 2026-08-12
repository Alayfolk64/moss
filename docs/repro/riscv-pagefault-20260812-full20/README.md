# QEMU RISC-V 完整流程二十轮记录

## 结论

重新计数后的 20 轮均已运行至明确终态：19 轮完整通过，1 轮超时。

- 20/20 出现 `Boot HART ID`、`MOSS_SMP online=8`、cagent 结束和
  `BUILDSTORM_BEGIN mode=multi`。
- 19/20 出现 `BUILDSTORM_COMPILE mode=multi ok=true` 和 buildstorm 结束，
  QEMU 自然退出且退出码为 0。
- 第 11 轮在 buildstorm 编译 `ax-api` 时停止输出，达到 1200 秒上限后由
  `timeout` 发送 SIGTERM，退出码为 124。它不是完整通过。
- 20 份日志中均未出现 `StorePageFault`、`LoadPageFault`、
  `InstructionPageFault`、kernel panic、重复发布正式根页表断言或错误 root
  安装断言。因此原先的内核缺页崩溃本次没有复现，但还不能把第 11 轮的卡住
  解释成已经解决。

逐轮结构化数据见 `summary.tsv`，原始串口输出为 `full01.log` 至
`full20.log`，runner 退出状态与耗时为对应的 `fullNN.status`。

## 固定测试条件

```text
时间范围：2026-08-12 22:29:35 至 2026-08-13 00:10:35（Asia/Shanghai）
并发方式：两个独立 QEMU 进程，各自串行运行 10 轮
宿主 CPU：AMD Ryzen 9 7940HX with Radeon Graphics，32 logical CPUs
宿主内存：30 GiB，运行检查时 swap 使用为 0
QEMU：QEMU emulator version 10.0.2
Guest：16 GiB，8 harts
根文件系统：sdcard-rv.img，使用 -snapshot，不回写源镜像
单轮上限：1200 秒，TERM 后保留 30 秒再强制终止
```

固定产物：

```text
8d9a67a767e0b8e9106beee62d69c3cc3831c2fbda4465de7ab66af3a3e38846  kernel-rv
7d623c123b7fb006e857a592c606388849377fc0f416e157ffd1892db003cde5  sdcard-rv.img
```

`kernel-rv` 大小为 4,813,752 字节；`sdcard-rv.img` 大小为
15,032,385,536 字节。测试后没有残留 QEMU 或 runner 进程。

## 实际运行命令

每轮的核心命令如下：

```bash
timeout --signal=TERM --kill-after=30s 1200s \
  qemu-system-riscv64 \
  -machine virt \
  -kernel kernel-rv \
  -m 16G \
  -nographic \
  -smp 8 \
  -bios default \
  -snapshot \
  -drive file=sdcard-rv.img,if=none,format=raw,id=x0 \
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
  -no-reboot \
  -device virtio-net-device,netdev=net \
  -netdev user,id=net \
  -rtc base=utc
```

这里 `-snapshot` 让 guest 的磁盘写入进入临时层，每轮都以同一个未被前一轮
修改的 `sdcard-rv.img` 为基线。两个并发 QEMU 是两个独立进程；每个 guest 内部
各有 8 个 hart，并不是把一次 guest 测试拆成两条线程。

## 汇总

完整通过的 19 轮：

```text
QEMU 总耗时：最短 475 s，中位数 542 s，最长 575 s，平均 536.21 s
buildstorm： 最短 442.66 s，中位数 508.50 s，最长 542.06 s，平均 502.64 s
```

20 轮 Boot HART 分布：

```text
hart 0: 5
hart 1: 3
hart 2: 2
hart 3: 1
hart 4: 1
hart 5: 1
hart 6: 4
hart 7: 3
```

这覆盖了全部 hart 0–7，并且每轮都最终达到 `MOSS_SMP online=8`。它验证了修复
没有依赖固定由 hart 0 成为 Boot HART。

## 第 11 轮异常现场

第 11 轮的 Boot HART 为 0。它完成了 SMP 上线、cagent 和
`BUILDSTORM_BEGIN`，最后的 guest 输出为：

```text
Compiling ax-mm v0.5.28
Compiling ax-sync v0.5.28
Compiling ax-api v0.7.4
qemu-system-riscv64: terminating on signal 15 from pid 22228 (timeout)
```

静默期间宿主仍有约 18 GiB 可用内存，swap 使用为 0，I/O wait 为 0；短窗口
采样中该 QEMU 仅使用约 5%–7% CPU，而同一时间的另一实例可以使用约 800% CPU。
这些证据说明它不是宿主 OOM 或 swap 抖动，更像 guest 内部等待或卡住。当前日志
不足以把卡住定位为页表问题，也没有证据表明它与 Boot HART 0 有因果关系：第 12、
16、19、20 轮同样由 hart 0 启动并完整通过。

## 计数边界

用户要求重新开始时，最初尚未完成的 `full01.log` 和 `full02.log` 被移动到
`aborted-20260812-222923/`。它们没有计入这里的 20 轮结果，也没有覆盖或删除。

## Rebase 到 dev 后复验

上述修复随后 rebase 到远端 `dev` 的 `fc62c65a`。最终源码依次通过：

```bash
cargo clean --target-dir target
make kernel-rv
make kernel-vf2
make kernel-la
cargo fmt --all -- --check
git diff --check origin/dev..HEAD
```

冷构建发现 rebase 前的 `target` 缓存中残留旧版 `lock_api` 元数据；清理仓库
`target/` 后三个目标均构建成功。rebase 后的 RISC-V 镜像为：

```text
b0d93e553c90610d40f94f097d3092909eb57ed4a11a6e3fb4d358f821c6a138  kernel-rv
```

该镜像又执行了一轮 16 GiB、8 hart 的完整 QEMU 流程：Boot HART 为 6，cagent
结束，buildstorm 得到 `ok=true`，编译耗时 501.12 秒；QEMU 总耗时 535 秒并以
状态 0 自然退出。日志中没有 Page Fault、panic 或页表断言。

首次复验调用从 PTY 后台启动，`-nographic` 读取终端时触发 job-control，QEMU 和
`timeout` 都处于 stopped 状态且日志为零字节。该进程已终止且不计入复验结果；
有效复验把 stdin 设为 `/dev/null`，排除了终端作业控制影响。

### 推送前额外并发复验

为了继续扩大 rebase 后镜像的并发覆盖，又同时启动两台 QEMU。两轮使用相同镜像，
实际可复现命令如下；stdin 固定为 `/dev/null` 可避免 `-nographic` 的 PTY
job-control 问题：

```bash
run_full() {
  log_path="$1"
  timeout --signal=TERM --kill-after=30s 1200s \
    qemu-system-riscv64 \
      -machine virt -kernel kernel-rv -m 16G -nographic -smp 8 \
      -bios default -snapshot \
      -drive file=sdcard-rv.img,if=none,format=raw,id=x0 \
      -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
      -no-reboot \
      -device virtio-net-device,netdev=net \
      -netdev user,id=net -rtc base=utc \
      < /dev/null > "$log_path" 2>&1
}

run_full docs/repro/riscv-pagefault-20260812-full20/rebase-dev-full03.log & pid03=$!
run_full docs/repro/riscv-pagefault-20260812-full20/rebase-dev-full04.log & pid04=$!
wait "$pid03"; rc03=$?
wait "$pid04"; rc04=$?
echo "rc03=$rc03 rc04=$rc04"
```

结果如下：

| 轮次 | Boot HART | QEMU 退出码 | 总耗时 | buildstorm | 异常扫描 |
| --- | ---: | ---: | ---: | --- | ---: |
| rebase-dev-full03 | 0 | 0 | 586 s | `ok=true`, 551.88 s, 8 cores | 0 |
| rebase-dev-full04 | 0 | 0 | 582 s | `ok=true`, 548.22 s, 8 cores | 0 |

两轮均识别 8 个 hart，cagent 和 buildstorm 测试组都到达 `END`，QEMU 自然退出。
异常扫描包括 Page Fault、panic、页表根断言、timeout 和 QEMU error。full03 的
`fs-readwrite` 与 `cpu` 两条通过信息在串口输出中发生字符交织，但两项结果仍可
辨认，随后 cagent 正常结束；这属于并发串口输出未按整行原子化，不是测试失败。

因此，rebase 到 `dev` 后同一 SHA256 镜像共完成 3 轮完整流程，结果为 3/3 通过；
其中 Boot HART 覆盖 6 和 0。
