# 星光二号板 VF2 启动指令

通过串口进入 U-Boot，用 TFTP 加载 MOSS 内核，使用 SD 卡上的根文件系统启动。
宿主机命令均从 `moss` 项目根目录执行。设备名按[板卡指南](README.md)替换为测试电脑的实际设备名。

接好 USB 串口，先在宿主机终端连接板子：

```bash
picocom -b 115200 /dev/ttyACM0
```

`-b 115200` 设置波特率，`/dev/ttyACM0` 是串口设备。保持连接，后续构建和网络配置在另一个宿主机终端执行。
退出 picocom 时先按 `Ctrl-A`，再按 `Ctrl-X`。

| 项目 | 配置 |
| --- | --- |
| PC 直连网卡 | `enp3s0` |
| PC IP | `192.168.89.1` |
| 板卡 IP | `192.168.89.2` |
| TFTP 目录 | `/srv/tftp` |
| 根分区 | `/dev/mmcblk1p3` |

## 1. 宿主机：构建并部署

```bash
make site-vf2
sudo cp build/site-vf2/moss-vf2.bin /srv/tftp/moss-vf2.bin
```

第一条构建 4 核、4 GiB 的 VF2 交互镜像，第二条将内核复制到 TFTP 目录。
启动后进入 MOSS shell，比赛脚本由用户手动执行。

## 2. 宿主机：配置网络和 TFTP

网线直连 PC 与 VF2，在宿主机执行：

```bash
sudo ip addr replace 192.168.89.1/24 dev enp3s0
sudo ip link set enp3s0 up
```

第一条设置 PC 地址，`/24` 是子网前缀长度；第二条启用网卡。

检查 TFTP 端口：

```bash
sudo ss -lunp 'sport = :69'
```

`-lunp` 显示正在监听的 UDP 端口及进程，过滤条件只查看 69 端口。
若没有服务监听 `192.168.89.1:69` 或 `0.0.0.0:69`，执行：

```bash
sudo /usr/sbin/in.tftpd --listen --user tftp --address 192.168.89.1:69 --secure /srv/tftp
```

该命令以 `tftp` 用户启动服务，监听指定地址，并使用 `/srv/tftp` 作为文件目录。

## 3. 板端：进入 U-Boot 并启动

保持串口连接，给 VF2 上电；如果已经进入原厂 Linux，先执行 `sudo reboot` 重启。
看到自动启动倒计时后连续按空格，进入 `StarFive #`，逐行执行：

```text
setenv ipaddr 192.168.89.2
setenv serverip 192.168.89.1
setenv netmask 255.255.255.0
tftpboot 0x40200000 moss-vf2.bin
fdt addr 0x42200000
fdt resize 4096
fdt set /chosen bootargs "root=/dev/mmcblk1p3 rw"
go 0x40200000
```

前三条设置板卡和 TFTP 网络；`tftpboot` 将内核加载到 `0x40200000`。
`fdt addr` 选择固件保留在 `0x42200000` 的 DTB，`fdt resize 4096` 为修改预留空间，
`fdt set` 指定可读写的根分区，`go` 启动内核。这些设置只用于当次启动，无需 `saveenv`。

根分区使用[烧录教程](星光二号板烧录教程.md)写入的第三分区：宿主机读卡器上的 `/dev/sdb3`，
装回 VF2 后对应 `/dev/mmcblk1p3`。

## 4. MOSS：校准时间

看到 `starry:~#` 后，在另一个**宿主机终端**确认电脑时间正确，再执行：

```bash
date -u '+date -u -s "%Y-%m-%d %H:%M:%S"'
```

这条命令按宿主机的 UTC 时间生成校时命令。复制输出的完整一行，保留双引号，
粘贴到 MOSS 的 `starry:~#` 下执行；其中 `-u` 表示 UTC，`-s` 设置时间。
校时完成后即可手动执行命令和比赛脚本。

## 5. MOSS：常用检查

```sh
uname -m
nproc
cat /sys/devices/system/cpu/online
cat /proc/cpuinfo
cat /proc/meminfo
free -h
mount
df -h /
ip addr
```

这些命令依次查看架构、CPU、内存、文件系统挂载和网络地址。
当前目标配置是 4 核、4 GiB 内存：

- `uname -m` 应为 `riscv64`。
- `nproc` 应为 `4`。
- CPU online 应为 `0-3`。

## 6. MOSS：运行比赛脚本

使用包含 `/glibc` 比赛脚本的根文件系统，在 MOSS 交互 shell 中运行 CAgent：

```sh
cd /glibc
bash ./cagent_testcode.sh
```

运行 BuildStorm：

```sh
cd /glibc
bash ./buildstorm_testcode.sh
```


`cd` 进入比赛目录，`bash` 执行对应测试脚本。
镜像中已安装的工具可这样查看版本：

```sh
git --version
vim --version
gcc --version
rustc --version
```

## 7. MOSS：结束操作

在启动后进入的 MOSS shell 中执行：

```sh
sync
exit
```

`sync` 刷新写入，`exit` 结束交互 shell，内核随后同步、卸载文件系统并关机。
等待关机完成后，按 VF2 Reset 再次启动；要返回原厂 Linux，让 U-Boot 自动启动即可。

其他故障处理见[维护与排障](../../history/boards/星光二号板维护与排障.md)。
