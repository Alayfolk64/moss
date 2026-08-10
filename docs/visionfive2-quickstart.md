# VisionFive 2 上板常用指令

本文记录在 VisionFive 2 上构建、传输和启动 Moss 的常用操作。以下命令按当前测试环境编写，设备名变化时请相应替换。

| 项目 | 当前配置 |
| --- | --- |
| 串口 | `/dev/ttyACM0`，115200 波特率 |
| PC 网口 | `enp108s0` |
| PC IP | `192.168.1.2` |
| 板端 U-Boot IP | `192.168.1.10` |
| TFTP 目录 | `/srv/tftp` |
| SD 卡 | PC 端通常为 `/dev/sdb`，U-Boot 中为 `mmc 1` |
| 内核加载地址 | `0x40200000` |

> 写卡前必须用 `lsblk` 再次确认 SD 卡设备，避免覆盖 PC 磁盘。

## 1. 接线

- USB 串口连接 PC，用于查看日志和输入命令。
- 网线直连 PC 与开发板，用于 U-Boot TFTP 传输内核和 Moss 网络通信。
- 插入写有 Moss rootfs 的 SD 卡。

## 2. 构建内核

在 Moss 仓库根目录执行：

```bash
make kernel-vf2
```

生成文件：

```text
moss-vf2.bin
```

VF2 上的功能测试统一使用该内核。`scripts/run_rv.sh` 用于 PC 上启动 QEMU，不用于 VF2 实板。

## 3. 配置网络和 TFTP

配置 PC 网口：

```bash
sudo ip addr replace 192.168.1.2/24 dev enp108s0
sudo ip link set enp108s0 up
ip addr show enp108s0
```

复制待启动内核：

```bash
sudo cp moss-vf2.bin /srv/tftp/kernel
```

查看 TFTP 服务：

```bash
ps -ef | grep '[i]n.tftpd'
```

若服务尚未运行：

```bash
sudo /usr/sbin/in.tftpd --listen --user tftp \
  --address 192.168.1.2:69 --secure /srv/tftp
```

## 4. 写入 SD 卡 rootfs

确认设备：

```bash
lsblk -o NAME,SIZE,MODEL,TRAN,FSTYPE,MOUNTPOINTS
```

当前 `sdcard-rv.img` 是整盘 ext4 镜像，应写入整张卡而不是分区：

```bash
sudo umount /dev/sdb* 2>/dev/null || true
sudo dd if=sdcard-rv.img of=/dev/sdb bs=16M \
  iflag=fullblock oflag=direct status=progress conv=fsync
sudo e2fsck -fy /dev/sdb
sync
```

写入完成后再将 SD 卡插回开发板。

## 5. 打开串口和进入 U-Boot

在 PC 上启动 minicom：

```bash
sudo minicom -D /dev/ttyACM0 -b 115200 -o
```

常用按键：

- `Ctrl-A` 后按 `Z`：查看帮助。
- `Ctrl-A` 后按 `X`：退出 minicom。
- 开发板复位或上电后，在倒计时期间按任意键：进入 U-Boot。
- 若已开始自动网络启动，按 `Ctrl-C`，直到出现 `StarFive #`。

## 6. 通过 TFTP 加载并启动 Moss

在 `StarFive #` 下执行：

```text
setenv ipaddr 192.168.1.10
setenv serverip 192.168.1.2
tftpboot 0x40200000 kernel
go 0x40200000
```

`setenv` 只对本次启动有效，不需要执行 `saveenv`。

## 7. 在 U-Boot 中查看 SD 卡

选择并检测 SD 卡：

```text
mmc dev 1
mmc rescan
mmc info
```

当前整盘 ext4 镜像：

```text
fstype mmc 1
ls mmc 1 /
ls mmc 1 /glibc
```

若使用带分区表的旧镜像：

```text
part list mmc 1
fstype mmc 1:1
ls mmc 1:1 /
```

## 8. 在 PC 上查看或复制 SD 卡文件

当前整盘 ext4 镜像：

```bash
sudo mkdir -p /mnt/moss-sd
sudo mount /dev/sdb /mnt/moss-sd
ls /mnt/moss-sd
sudo cp <本地文件> /mnt/moss-sd/<目标路径>
sync
sudo umount /mnt/moss-sd
```

若镜像带分区表，将 `/dev/sdb` 改为 `/dev/sdb1`。

## 9. Moss 启动后的常用检查

```sh
ls /
ls /glibc
cat /proc/cpuinfo
cat /proc/meminfo
mount
```

在 Moss Shell 中运行 cagent：

```sh
cd /glibc
bash ./cagent_testcode.sh
```

运行 buildstorm：

```sh
cd /glibc
bash ./buildstorm_testcode.sh
```

验证 Moss 向 PC 发送 TCP 数据，先在 PC 执行：

```bash
nc -l 192.168.1.2 19000
```

再在 Moss 中执行：

```sh
echo MOSS_VF2_TX_OK | busybox nc -w 3 192.168.1.2 19000
```

验证 PC 向 Moss 发送数据，先在 Moss 中执行：

```sh
busybox nc -l -p 19001
```

再在 PC 执行：

```bash
printf 'MOSS_VF2_RX_OK\n' | nc -w 3 192.168.1.10 19001
```

## 10. 常见问题

- `Card did not respond to voltage select! : -110` 通常来自未就绪的 `mmc 0`；SD 卡应使用 `mmc 1`。
- `serverip not set`：重新执行 `setenv serverip 192.168.1.2`。
- 一个网口 PHY 超时不代表 TFTP 一定失败；成功时通常会切换到 `ethernet@16040000`。
- `tftpboot` 失败时检查网线、PC 网口 IP、TFTP 服务和 `/srv/tftp/kernel`。
- Moss 已支持 VF2 物理网口的 TCP/IP 通信；`scp` 仍取决于 rootfs 是否提供 SSH 客户端或服务端。更新内核建议继续使用 U-Boot TFTP。
