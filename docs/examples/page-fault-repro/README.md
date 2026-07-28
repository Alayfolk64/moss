# Moss RISC-V 缺页全流程复现

这不是示意代码。`pf-repro.c` 会动态链接测试镜像中的 RISC-V
glibc，实际调用 glibc 的 `write(2)`、`mmap(2)`、`munmap(2)` 和
`_exit(2)`，然后用一条普通的 RISC-V `sb` 指令首次写入懒映射页。

运行：

```bash
docs/examples/page-fault-repro/run-moss.sh
```

默认要求：

- 仓库根目录存在 `sdcard-rv.img`；
- `../testsuits-for-oskernel/sdcard/riscv/glibc/lib` 中存在
  `libc.so.6` 和动态加载器；
- PATH 中存在 `riscv64-linux-musl-gcc`、
  `riscv64-linux-musl-objdump`、`qemu-system-riscv64` 和
  `debugfs`。

路径不同时可以设置 `SDCARD_RV_IMG`、`TESTSUITES_DIR`、
`GLIBC_DIR`、`RISCV_CC`、`RISCV_OBJDUMP` 或 `OUTPUT_DIR`。

脚本执行的动作是：

1. 用现有交叉编译器直接链接测试镜像里的 glibc；
2. 保存 ELF 解释器、动态依赖、glibc 符号和触发缺页指令的证据；
3. 用 `fault-repro-trace` 构建 Moss；
4. 复制基础 ext4 镜像到临时目录，仅向副本写入 6736 字节的程序；
5. 启动 QEMU，等到 shell 出现后自动运行 `/root/pf-repro`；
6. 等到程序退出码为 0 后关闭 QEMU，并删除临时镜像。

基础 `sdcard-rv.img` 不会被写入。实验输出默认保存在
`docs/profiling/riscv-page-fault-repro-manual/`。

成功日志应证明以下严格顺序：

```text
mmap(... MAP_PRIVATE|MAP_ANONYMOUS ...) 返回
pte_after_mmap=Err(NotMapped)
0x1043c: sb a4,0(a5)
scause=15 (StorePageFault), stval=0x3000000000, sepc=0x1043c
分配并清零匿名物理页
安装 READ|WRITE|USER PTE，刷新 TLB
resume_same_ip=0x1043c
同一条 sb 重试成功，用户态读到 0x5a，进程退出码为 0
```

2026-07-28 的已验证运行保存在
`docs/profiling/riscv-page-fault-repro-20260728/`。
