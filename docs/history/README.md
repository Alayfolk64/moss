# 历史问题与修复

这里保留问题定位、修复过程和当时的验收结果。日常启动请用[操作指南](../guides/README.md)。文中引用的实板验收、GDB 和 xv6 等开发脚本已删除，旧命令仅作历史记录。

## 板卡

- [原生 GDB 双架构实现与验收指南](boards/原生GDB双架构实现与验收指南.md)

- [星光二号板分区镜像烧录教程](boards/星光二号板分区镜像烧录教程.md)
- [星光二号板维护与排障](boards/星光二号板维护与排障.md)
- [龙芯文件复制与串口长输入排查](boards/龙芯二号板文件复制与串口长输入排查.md)

- [双板串口输入延迟修复与验收](boards/双板串口输入延迟修复与验收.md)
- [VF2 网卡驱动实现与验收](boards/星光二号板网卡驱动实现与验收.md)
- [龙芯第一阶段设计](boards/龙芯二号板第一阶段设计.md)
- [龙芯全程调试记录](boards/龙芯二号板调试状态报告-20260808.md)
- [龙芯 Linux 6.12 使用与验收](boards/龙芯二号板Linux-6.12使用与验收报告-20260811.md)
- [龙芯现场赛软件验收](boards/龙芯二号板现场赛软件验收报告-20260812.md)

## 内存和多核

- [RISC-V 非零 Boot HART 缺页排查](repro/riscv-pagefault-20260812-run01/README.md)
- [完整流程二十轮记录](repro/riscv-pagefault-20260812-full20/README.md)
- [Linux 式页表分层修复验证](repro/riscv-pagefault-20260812-linux-pt-fix/README.md)
- [多核调度与构建性能优化迁移](smp-scheduler-optimization-migration.md)

部分板卡报告引用的 `hardware-*.log`、`static-*.log` 等文件未随仓库提供。已有日志见[原始记录](../evidence/README.md)。
