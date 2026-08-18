# VF2 开发板手动启动命令

本文只记录 VisionFive 2 通过 `picocom`、U-Boot 和 TFTP 手动启动最新 MOSS 的主流程。每次启动前重新构建无看门狗的 `site-init` 镜像，不运行自动验收脚本。只连接 VF2 时使用下面的默认主流程；电脑同时连接龙芯和 VF2 时，改用文末的双板网络配置。

## 主流程

### 1. 构建并部署最新镜像

进入仓库并重新构建：

```bash
cd /home/wuxun/Projects/moss-dev
make site-vf2
```

该目标固定选择 `vf2 site-init`，不会加入 `vf2-watchdog`；板卡地址 `192.168.88.2/24` 和网关 `192.168.88.1` 已固化，不接受额外构建参数。

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
sudo ip address replace 192.168.88.1/24 dev enp3s0
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
setenv ipaddr 192.168.88.2
setenv serverip 192.168.88.1
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

## 查看本机网口名称

```text
ip -brief link
ip -brief address
会因电脑不同而网口不同
```

```text
e.g
zjr@F16:~/Data/moss$ ip -brief link
lo               UNKNOWN        00:00:00:00:00:00 <LOOPBACK,UP,LOWER_UP> 
enp3s0           UP             10:7c:61:6f:5f:58 <BROADCAST,MULTICAST,UP,LOWER_UP>
wlo1             UP             f0:20:ff:92:12:88 <BROADCAST,MULTICAST,UP,LOWER_UP> 
zjr@F16:~/Data/moss$ ip -brief address
lo               UNKNOWN        127.0.0.1/8 ::1/128 
enp3s0           UP             192.168.88.1/24
wlo1             UP             172.20.10.7/28 2409:8903:4b44:1093:2f36:728c:bad9:249a/64 2409:8903:4b44:1093:fd75:568b:677b:f00/64 fe80::33d3:541c:9ef9:1444/64 
```

## 一台电脑同时连接龙芯和 VF2

两块板同时连接时，不要让电脑的两个直连网口使用同一个网段，也不要让两块板使用同一个
IP。当前电脑按下表配置：

| 板卡 | 电脑直连网口 | 电脑地址 | 板卡地址 |
| --- | --- | --- | --- |
| 龙芯 2K1000LA | `enp3s0` | `192.168.88.1/24` | `192.168.88.2/24` |
| VF2 | `enx6c1ff75c0e36` | `192.168.89.1/24` | `192.168.89.2/24` |

`enx6c1ff75c0e36` 是当前 USB 网卡的名称。更换 USB 网卡后名称可能变化，先用
`ip -brief link` 确认实际名称。

### 1. 配置电脑的两个直连网口

```bash
# 龙芯网口保持 192.168.88.0/24
sudo ip address replace 192.168.88.1/24 dev enp3s0
sudo ip link set dev enp3s0 up

# 清除 VF2 网口的旧 IPv4 地址，再配置独立的 192.168.89.0/24
sudo ip address flush dev enx6c1ff75c0e36 scope global
sudo ip address add 192.168.89.1/24 dev enx6c1ff75c0e36
sudo ip link set dev enx6c1ff75c0e36 up
```

检查地址和路由：

```bash
ip -brief address show enp3s0
ip -brief address show enx6c1ff75c0e36
ip route get 192.168.88.2
ip route get 192.168.89.2
```

正确结果应分别显示：

```text
192.168.88.2 dev enp3s0 src 192.168.88.1
192.168.89.2 dev enx6c1ff75c0e36 src 192.168.89.1
```

两个直连网口都不要配置默认网关。电脑的默认路由继续使用 Wi-Fi 或其他公网网口。

### 2. VF2 使用新网段进行 TFTP

当前电脑上的 TFTP 服务监听 `0.0.0.0:69`，同一个 `/srv/tftp` 可以同时服务两个网段，
不需要再启动第二个 TFTP 进程。先检查：

```bash
sudo ss -lunp | grep ':69'
```

当前应看到 `0.0.0.0:69`。进入 VF2 的 `StarFive #` 后执行：

```text
setenv ipaddr 192.168.89.2
setenv serverip 192.168.89.1
setenv netmask 255.255.255.0
printenv ipaddr serverip netmask
ping 192.168.89.1
tftpboot 0x40200000 moss-vf2-nowatchdog.bin
fdt addr 0x42200000
fdt resize 4096
fdt set /chosen bootargs "root=/dev/mmcblk1p3 rw"
go 0x40200000
```

这些是本次启动使用的 U-Boot 临时值，不执行 `saveenv`。

### 3. 进入 MOSS 后修改 VF2 运行时地址

`make site-vf2` 构建的镜像默认仍是 `192.168.88.2/24`。进入 `starry:~#` 后立即通过串口执行：

```sh
ip address del 192.168.88.2/24 dev eth0
ip address add 192.168.89.2/24 dev eth0
ip route replace default via 192.168.89.1
ip -brief address show eth0
ip route
```

运行时修改不会写入镜像，VF2 每次重新加载 MOSS 后都要执行一次。修改完成后，VF2 使用
`192.168.89.2`，龙芯继续使用 `192.168.88.2`，两块板可以同时连接且不会发生地址或路由冲突。

### 4. VF2 需要通过电脑访问公网时

只进行 TFTP 启动不需要本节。VF2 还需要通过电脑访问公网时，为
`192.168.89.0/24` 单独增加转发和 NAT 规则：

```bash
sudo sysctl -w net.ipv4.ip_forward=1

sudo iptables -C FORWARD -s 192.168.89.0/24 -j ACCEPT || \
  sudo iptables -I FORWARD 1 -s 192.168.89.0/24 -j ACCEPT

sudo iptables -C FORWARD -d 192.168.89.0/24 \
  -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT || \
  sudo iptables -I FORWARD 1 -d 192.168.89.0/24 \
    -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT

sudo iptables -t nat -C POSTROUTING -s 192.168.89.0/24 -j MASQUERADE || \
  sudo iptables -t nat -A POSTROUTING -s 192.168.89.0/24 -j MASQUERADE
```

这三条 `iptables` 命令会先检查规则，重复执行不会添加重复项。没有持久化这些规则时，
电脑重启后需要重新执行。


## git 使用过程中 ssh 配置方法

```text
配置SSH
1.ssh-keygen -t ed25519 -C "你的GitHub邮箱"
一路回车
2.查看公钥 cat ~/.ssh/id_ed25519.pub
3.github添加
头像
 ↓
Settings
 ↓
SSH and GPG keys
 ↓
New SSH key
4.测试
ssh -T git@github.com
预期输出 Hi username! You've successfully authenticated
```
