# LS2K1000LA 2025 现场赛软件验收报告

更新日期：2026-08-12

## 1. 结论

官方 `on-site-final-2025` 分支列出的 9 个计分点，已经在龙芯 2K1000LA 星云版真机、MOSS、
物理 GMAC 和物理 SATA `/dev/sda3` 上全部通过。测例定义取自仓库提交
`9ea291d8535774680e88f4e6bf725d7ba156e752`，不是根据印象自行改写。

| 题目 | 真机结果 |
|---|---|
| Git help | Git 2.47.3，帮助输出非空 |
| Git init/add/commit/log | 提交 `0b5fa64 add-README`，各命令返回 0 |
| Git clone | 从 Gitee 官方 xv6 仓库接收 7780 个对象、17.49 MiB、4111 个 delta，`clone_rc=0` |
| Git push | 板上提交 `0ea3c78` 推送到私有验收仓库 `main`，`push_rc=0` |
| Git pull | 宿主机远端提交 `5477b08`，板上 fast-forward 拉回，`pull_rc=0`，新文本核对返回 0 |
| Vim help | Vim 9.1 帮助和版本输出正常 |
| Vim 编辑保存 | 真实进入 Vim 插入、保存、退出；保存的 C 文件再次由 GCC 编译并输出 `Hello from Vim` |
| GCC | GCC 14.2.0，帮助输出正常；C Hello World 编译和运行均返回 0 |
| rustc | rustc 1.83.0，帮助输出正常；Rust Hello World 编译和运行均返回 0 |

官方题目允许 GitHub、Gitlink、Gitee 三者任选其一。本次 Gitee 全量 clone 成功；同一现场出口到
GitHub 的浅 clone 曾在 TLS 读取阶段被中间链路断开，所以最终 push/pull 也统一使用 Gitee，避免
把外网出口差异误判成 MOSS 网络栈故障。

## 2. 实际运行环境

- 磁盘：`sda1` 约 11.8 GiB 原厂 Linux，`sda2` 12 GiB 原 MOSS 环境，`sda3` 6 GiB 官方
  Alpine LoongArch64 环境。
- MOSS 根：DT bootargs 为 `root=/dev/sda3`。
- 网络：MOSS 静态地址 `192.168.88.2/24`，宿主机有线口 `192.168.88.1/24`；宿主机通过
  IPv4 forwarding 和 NAT 转发到无线网。
- 启动：U-Boot/TFTP 交接阶段使用 30 秒硬件 watchdog；进入交互 shell 后立即停掉 watchdog，
  不会在输入命令时自动复位。
- 时钟：MOSS 启动后 `ntpd` 通过物理网络把 RTC 的 2022 年校准到 2026-08-12，HTTPS 证书校验
  因此正常。

## 3. 现场复现

宿主机先构建和部署交互镜像：

```bash
make kernel-la-2k1000-shell
sudo install -o tftp -g tftp -m 0644 \
  moss-2k1000la-shell.bin /srv/tftp/moss-shell.bin
python3 scripts/run_ls2k1000_serial_tftp.py moss-shell.bin \
  --root-device /dev/sda3 \
  --monitor-seconds 180 \
  --log /tmp/moss-onsite.log
```

进入 `starry:~#` 后，四类本地测试按官方题面直接执行即可。Git 网络题推荐浅 clone，避免现场
首次向空私库推送十多年完整历史；若 Gitee 禁止 shallow history push，可在已 clone 的工作树上
建立普通根提交后推送当前快照：

```bash
git clone --depth 1 https://gitee.com/oscomp/xv6-riscv.git xv6
cd xv6
git config user.name MOSS-LS2K1000LA
git config user.email moss-onsite@example.com
vi README
git add README
git commit -m 'update README on MOSS'
git remote add me https://gitee.com/你的账号/你的仓库.git
git checkout --orphan main
git add -A
git commit -m 'import xv6 and update README on MOSS'
git push -u me main
git pull me main
```

私有仓库认证只在当次实验中临时写入 `/root/.git-credentials`，验收后已删除；代码、文档和串口
日志中均未保存 token。

## 4. 本轮发现并修复的问题

1. 自动脚本原先固定 `root=/dev/sda2`。现在提供 `--root-device /dev/sda2|/dev/sda3`，并在 Linux
   交接前停止 udev 的私有挂载，避免两个内核同时持有同一 ext4。
2. MOSS 原串口路径只用 IRQ 唤醒 TTY，自动化一次写入整行会在同步回显期间淹没 UART FIFO；
   BusyBox ash 还会发送 ANSI `ESC[6n` 光标查询。runner 会回应 DSR；后续 UART 重构已经改为
   IRQ 立即搬运到 8 KiB 软件 RX 环，并删除了验收时使用的逐字符节流规避。
3. 旧退出流程只有 VFS `flush()`，全局持有的 lwext4 对象不会 Drop，因此没有执行
   `ext4_fs_fini()`，每次复位后超级块都显示 `not clean with errors`。现在增加幂等
   `FilesystemOps::shutdown()`：flush、写入 ext4 clean 状态、刷新块设备后才复位。

新退出流程已在真机写文件、`sync`、`exit`、自动复位后验证：Linux 不再打印 unchecked ext4
警告，`dumpe2fs` 为 `Filesystem state: clean`，`e2fsck -fn /dev/sda3` 五阶段全通过且返回 0，
写入文件的内容核对也返回 0。

## 5. 验收日志

当次完整原始日志保存在宿主机：

- `/tmp/moss-onsite-network-20260812.log`
- `/tmp/moss-clean-shutdown-validation-20260812.log`

最终镜像 SHA-256 为
`df60f76169c98f07a9b35e208342e83c0058c6c8751b9def2489fbb9ee6f1a23`。
