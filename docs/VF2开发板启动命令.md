# VF2 开发板手动启动命令

本文只记录 VisionFive 2 通过 `picocom`、U-Boot 和 TFTP 手动启动最新 MOSS 的主流程。每次启动前重新构建无看门狗的 `site-init` 镜像，不运行自动验收脚本。

## 主流程

### 1. 构建并部署最新镜像

进入仓库并重新构建：

```bash
cd /home/wuxun/Projects/moss-dev
make site-vf2
```

该目标固定选择 `vf2 site-init`，不会加入 `vf2-watchdog`；板卡地址 `192.168.1.10/24` 和网关 `192.168.1.2` 已固化，不接受额外构建参数。

将刚生成的镜像部署到 TFTP：

```bash
sudo install -o tftp -g tftp -m 0644 \
  build/site-vf2/moss-vf2.bin \
  /srv/tftp/moss-vf2-nowatchdog.bin

sha256sum \
  build/site-vf2/moss-vf2.bin \
  /srv/tftp/moss-vf2-nowatchdog.bin
```

两行应输出相同的 SHA-256。

### 2. 配置网络并打开串口

配置与 VF2 直连的网口：

```bash
sudo ip address replace 192.168.1.2/24 dev enp3s0
sudo ip link set dev enp3s0 up
```

打开串口：

```bash
picocom -b 115200 /dev/ttyACM0
```

退出 `picocom` 使用 `Ctrl-A`，再按 `Ctrl-X`。

### 3. 进入 U-Boot

保持 `picocom` 打开，给 VF2 上电或按 Reset。看到自动启动倒计时后连续按空格，进入：

```text
StarFive #
```

### 4. 加载并启动 MOSS

在 U-Boot 中逐行执行：

```text
setenv ipaddr 192.168.1.10
setenv serverip 192.168.1.2
setenv netmask 255.255.255.0
tftpboot 0x40200000 moss-vf2-nowatchdog.bin
fdt addr 0x42200000
fdt resize 4096
fdt set /chosen bootargs "root=/dev/mmcblk1p3 rw"
go 0x40200000
```

这里固定使用 SD 卡的 p3 Alpine rootfs。`site-init` 进入 `/root`，设置 `starry:~#` 提示符后启动 `/bin/sh`，不自动运行网络、服务或验收命令。

### 5. 退出 MOSS

操作完成后先同步文件系统，再重启：

```sh
sync
reboot -f
```

如果串口停止但没有重新进入固件，确认 `sync` 已经返回后按 VF2 Reset。想回到原厂 Debian 时，不要截停下一次 U-Boot 自动启动。

## 切换 rootfs

只需要修改 U-Boot 中的 `root=`：

```text
fdt set /chosen bootargs "root=/dev/mmcblk1p2 rw"
```

或者：

```text
fdt set /chosen bootargs "root=/dev/mmcblk1p4 rw"
```

p5 是 swap，不能作为 rootfs。

手动启动不要使用 `run_vf2_gdb_native.py`，也不要设置 `moss,watchdog-hard-deadline-secs`。

完整的分区、TFTP 和故障排查说明见 [VisionFive 2 上板、启动与维护指令](boards/星光二号板启动指令.md)。
