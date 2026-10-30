# 脚本说明

本目录只保留下面五个脚本。命令均在仓库根目录执行，编译和 QEMU 运行环境见[仓库 README](../README.md#环境构建与运行)。

| 脚本 | 用途 | 最后修改（2026 年） |
| --- | --- | --- |
| [run_rv.sh](run_rv.sh) | 编译 RISC-V 内核并运行决赛测试，保存控制台日志 | 8 月 13 日 |
| [run_la.sh](run_la.sh) | 编译 LoongArch 内核并运行决赛测试，保存控制台日志 | 8 月 19 日 |
| [update_vendor_checksum.py](update_vendor_checksum.py) | 修改 vendor 源码后，更新对应 crate 的文件校验值 | 8 月 15 日 |
| [summary.sh](summary.sh) | 从双架构 LTP 日志中提取各用例结果，按失败数排序 | 6 月 18 日 |
| [total.sh](total.sh) | 汇总一份 LTP 结果中的 passed、failed、broken、skipped、warnings | 6 月 18 日 |

## 运行决赛测试

先把对应架构的官方决赛镜像放到仓库根目录，分别命名为 `sdcard-rv.img`、`sdcard-la.img`。下载地址见[镜像和赛题](../docs/guides/README.md#镜像和赛题)。

```sh
bash scripts/run_rv.sh
```

执行 `make kernel-rv` 后启动 RISC-V QEMU，配置为 8 核、16 GiB，日志写入 `docs/output_rv.txt`。

```sh
bash scripts/run_la.sh
```

执行 `make kernel-la` 后启动 LoongArch QEMU，配置为 8 核、8 GiB，日志写入 `docs/output_la.txt`。

两种架构分别运行。脚本使用快照模式，退出后不回写原始镜像；再次运行会覆盖对应日志。这两个脚本均使用多核，内核会选择决赛入口，不能直接用于初赛。

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

原实板验收、GDB、xv6、性能采样及旧测试入口已删除。历史文档中的相关命令仅作开发记录，脚本可从 Git 历史查找。
