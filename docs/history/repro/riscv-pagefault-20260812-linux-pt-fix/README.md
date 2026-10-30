# Linux 式页表分层修复验证

## 修复内容

RISC-V 页表接口按职责拆分：

```text
write_boot_page_table(root)
    只给当前 hart 安装早期 BOOT_PT

publish_kernel_page_table(root)
    只由主核发布一次正式 kernel root

write_kernel_page_table(root)
    只把已经发布的正式 root 安装到当前 hart
```

`publish_kernel_page_table()` 使用 `compare_exchange(0, root)`，重复发布即使值相同也
会 panic。`write_kernel_page_table()` 会验证参数等于已发布 root；
`read_kernel_page_table()` 在正式 root 尚未发布时也会 panic，不再回退读取本核
`satp`。QEMU 和 VisionFive2 的早期启动统一使用 `write_boot_page_table()`。

## 静态和构建验证

```bash
git diff --check
make kernel-rv
make kernel-vf2
make kernel-la
```

三项目标均构建成功。最终 RISC-V 镜像：

```text
8d9a67a767e0b8e9106beee62d69c3cc3831c2fbda4465de7ab66af3a3e38846  kernel-rv
```

仓库全量 `cargo fmt --all -- --check` 会报告修复前就存在的无关文件差异，并建议
重排新增断言的换行。格式调整会改变 Rust 构建哈希，因此最终保留生成全测镜像的
精确源码；没有格式化或改写用户已有文件，`git diff --check` 通过。

## 二十轮短启动

固定使用上述同一个镜像，在 16 GiB、8 CPU 条件下运行二十轮。每轮均出现
`MOSS_SMP online=8`，页错误、panic、重复发布和错误正式 root 安装计数均为 0。

```text
Boot HART 分布：0、2、3、4、5、6
成功：20/20
```

原始日志为 `short01.log` 至 `short20.log`。

## 双实例并发全程验证

从同一个 `sdcard-rv.img` 复制两个独立根文件系统：

```bash
cp sdcard-rv.img sdcard-rv-concurrent-1.img
cp sdcard-rv.img sdcard-rv-concurrent-2.img
```

两个实例同时使用同一个 `kernel-rv`，各自配置为 16 GiB、8 CPU，并分别绑定自己
的 raw 根文件系统；没有使用 `-snapshot`。两个 QEMU 同时启动并自然运行到测试组
结束，进程退出状态均为 0。

| 实例 | Boot HART | CPU online | cagent | buildstorm | 异常 |
| --- | ---: | ---: | --- | --- | --- |
| 1 | 2 | 8 | 完成 | `ok=true`，540.86 s | 0 |
| 2 | 7 | 8 | 完成 | `ok=true`，546.73 s | 0 |

验收标志：

```text
#### OS COMP TEST GROUP END cagent-glibc ####
BUILDSTORM_COMPILE mode=multi ok=true ... cores=8 ... arch=riscv64
#### OS COMP TEST GROUP END buildstorm-glibc ####
```

原始日志：

```text
full-concurrent-1.log
full-concurrent-2.log
```

运行结束后两个根文件系统 inode 和 SHA256 均不同，证明它们是独立文件并分别保存
了各自的写入结果：

```text
inode 37775403  f32cc44717bd57219d973e99b23901cf0d74dc153cb170ab37d874954cdeadc2
inode 37775929  6243a84e505553702be554dd1464ff9c05836d84da0ba6f5036f33b73f743b6a
```

异常检查排除了正常启动信息 `Platform Reboot Device: syscon-reboot`；两个日志中
`Page Fault`、`panicked`、重复发布断言和错误 root 安装断言的实际计数均为 0。
