# 脚本说明

命令均在仓库根目录执行，编译和 QEMU 运行环境见[仓库 README](../README.md#环境构建与运行)。

| 脚本 | 用途 | 最后修改（2026 年） |
| --- | --- | --- |
| [run_pre_rv.sh](run_pre_rv.sh) | 编译 RISC-V 内核并运行初赛测试，1 核、1 GiB | 11 月 1 日 |
| [run_pre_la.sh](run_pre_la.sh) | 编译 LoongArch 内核并运行初赛测试，1 核、1 GiB | 11 月 1 日 |
| [run_final_rv.sh](run_final_rv.sh) | 编译 RISC-V 内核并运行决赛测试，8 核、16 GiB | 11 月 1 日 |
| [run_final_la.sh](run_final_la.sh) | 编译 LoongArch 内核并运行决赛测试，8 核、8 GiB | 11 月 1 日 |
| [update_vendor_checksum.py](update_vendor_checksum.py) | 修改 vendor 源码后，更新对应 crate 的文件校验值 | 8 月 15 日 |
| [summary.sh](summary.sh) | 从双架构 LTP 日志中提取各用例结果，按失败数排序 | 6 月 18 日 |
| [total.sh](total.sh) | 汇总一份 LTP 结果中的 passed、failed、broken、skipped、warnings | 6 月 18 日 |

## 运行初赛与决赛测试

先把对应阶段、对应架构的镜像放到仓库根目录，分别命名为 `sdcard-rv.img`、`sdcard-la.img`。镜像来源和四个脚本的运行命令见[初赛与决赛指南](../docs/guides/README.md#初赛与决赛测试)。

脚本先执行 `make kernel-rv` 或 `make kernel-la`，再启动 QEMU。同一架构的初赛与决赛脚本只改变 `-smp` 核数和 `-m` 内存：初赛均为 1 核、1 GiB，决赛 RV 为 8 核、16 GiB，LA 为 8 核、8 GiB。

`-snapshot` 使用快照模式，退出后不回写镜像；`-display none` 关闭图形窗口。`-chardev` 配置终端和日志，`-serial` 接入串口，`-mon` 共用终端提供 QEMU 控制台。日志分别写入 `docs/output_rv.txt`、`docs/output_la.txt`，再次运行同架构脚本会覆盖日志。退出 QEMU 时先按 `Ctrl+A`，再按 `X`。

## 更新 vendor 校验值

```sh
python3 scripts/update_vendor_checksum.py vendor/axfs
```

`vendor/axfs` 是要更新的 crate 路径，换成实际修改的目录。脚本重算该目录的文件 SHA-256，更新 `.cargo-checksum.json` 的 `files`，保留 `package` 字段，并列出新增、删除和变化的文件。

## 整理 LTP 日志

这两个脚本处理初赛 LTP 日志，不用于统计 CAgent 或 BuildStorm 得分。

将 RV、LA 的 LTP 控制台日志分别放到 `docs/output_rv.txt`、`docs/output_la.txt`，两份文件都需要存在，然后执行：

```sh
bash scripts/summary.sh
```

输出 `docs/summary_rv.txt` 和 `docs/summary_la.txt`，保留包含 `RUN LTP CASE` 和 `Summary` 的用例结果，并按失败数从多到少排序。缺少 Summary 的用例不计入，不能把统计值当作完整测试覆盖率。

```sh
bash scripts/total.sh
```

默认汇总 `docs/summary_rv.txt`，写入 `docs/total.txt`。要汇总 LA 并单独保存：

```sh
bash scripts/total.sh docs/summary_la.txt docs/total_la.txt
```

第一个参数是输入文件，第二个参数是输出文件。输出会覆盖同名文件。
