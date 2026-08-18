# VisionFive 2 现场赛网络使用说明

本文面向 VisionFive 2（VF2）现场赛使用，记录受限赛事局域网下的接线、电脑双网口配置、MOSS 构建、TFTP 启动、网络功能验收、NAT 隔离和网段冲突切换方法。默认值来自 2026-08-18 在 `F16` Ubuntu 电脑上的实测环境。

## 1. 结论和使用边界

现场推荐使用两个彼此独立的以太网接口：

```text
赛事 RJ45 墙口
      │ 网线 1
      ▼
电脑网口 A（赛事局域网，由组委会配置或 DHCP）

电脑网口 B（VF2 私网，默认 192.168.88.1/24）
      │ 网线 2
      ▼
VF2 GMAC1（默认 192.168.88.2/24）

电脑 USB ───────── VF2 USB 串口
```

这套连接支持：

- 电脑访问赛事允许的评测平台和代码仓库；
- U-Boot 从电脑上的 TFTP 服务加载 MOSS；
- MOSS 与电脑进行 TCP、UDP、HTTP 和双向数据传输测试；
- VF2 串口控制和 SD 卡 rootfs 启动；
- 赛事局域网与 VF2 私网保持隔离。

默认情况下，MOSS 不能通过电脑访问赛事局域网。如果赛题明确要求 MOSS 自己访问赛事服务器，必须先向组委会确认是否允许 VF2 作为额外终端接入，以及应使用的 IP、网关、DNS 和端口。未经明确允许，不要使用 NAT、网桥、代理、SSH 隧道或热点把 VF2 接入赛事网络。

## 2. 赛事网络规则

根据赛方通知：

- 比赛期间禁止访问互联网及任何非赛事指定网络；
- 只允许访问组委会指定的评测平台、代码仓库和赛事必要服务；
- 允许访问的域名、IP 和端口由组委会统一配置；
- 禁止使用 VPN、代理、SSH 隧道、热点和移动网络绕过隔离；
- 现场可能启用屏蔽器和视频监控；
- 指定平台包括希冀评测平台 `https://course.educg.net/` 和希冀 GitLab `https://gitlab.eduxiji.net/`。

比赛时由电脑访问这些指定平台。文档或历史配置中的 GitHub、公网软件仓库、公共 NTP 和公共 DNS 不能作为现场依赖。

## 3. 硬件准备

至少准备：

- 两根网线；
- 一个 USB 转 RJ45 网卡；
- VF2 串口线；
- 必要时准备 USB Hub；
- 已写好并验证的多分区 SD 卡；
- 已在本机缓存好的 Rust 工具链、源码和构建依赖。

当前 VF2 私网接口固定使用 `enp3s0`。现场插入 USB 转网口后会出现另一个接口，常见名字为 `enx...` 或新的 `enp...`。必须通过插拔和 `ip -brief link` 确认赛事网口，避免误改 `enp3s0`。

保持 `enp3s0` 直连 VF2，把新出现的 USB 转网口用于赛事 RJ45。

## 4. 赛前关闭 VF2 NAT 和其他出口

这台电脑曾安装并启用 `moss-vf2-network.service`，它会开启 IP 转发并通过 nftables 将 VF2 流量 NAT 到默认上联网口。比赛前应移除：

```bash
cd /home/zjr/Data/moss
sudo scripts/setup_vf2_network.sh uninstall
sudo sysctl -w net.ipv4.ip_forward=0
```

`uninstall` 会删除 MOSS 专用 nftables NAT 表和持久化 systemd 配置，但当前脚本不会立即把运行期 `net.ipv4.ip_forward` 改回 `0`，所以第二条 `sysctl` 命令不能省略。

检查结果：

```bash
systemctl is-enabled moss-vf2-network.service 2>/dev/null
systemctl is-active moss-vf2-network.service 2>/dev/null
sysctl net.ipv4.ip_forward
sudo nft list table ip moss_vf2
```

预期：

- 服务为 `disabled`、`inactive` 或 `not-found`；
- `net.ipv4.ip_forward = 0`；
- 查询 `moss_vf2` 表时报不存在，这是正常结果。

现场关闭 Wi-Fi：

```bash
nmcli radio wifi off
```

同时退出 VPN、系统代理、SSH 隧道、手机热点和 USB 网络共享。比赛期间不要执行：

```bash
sudo scripts/setup_vf2_network.sh install
sudo scripts/setup_vf2_network.sh apply
```

关闭 NAT 不影响 VF2 与电脑之间的 TFTP、HTTP、TCP 或 UDP 通信。

## 5. 接入赛事网并识别两个网口

建议按以下顺序操作：

1. 先不要连接 VF2 网线；
2. 将赛事 RJ45 通过 USB 转网口接入电脑；
3. 等待组委会 DHCP 或按要求完成赛事网络配置；
4. 查看赛事网口名称、地址和路由；
5. 再连接 `enp3s0` 与 VF2。

检查：

```bash
ip -brief link
ip -4 -brief address
ip -4 route
```

正常状态应类似：

```text
enp3s0    UP  192.168.88.1/24      # VF2 私网
enx...     UP  <赛事分配的地址>      # 赛事局域网
```

路由应满足：

```text
192.168.88.0/24 dev enp3s0
default via <赛事网关> dev <赛事网口>
```

不要给 VF2 私网口配置默认网关。不要在赛事网口上配置 `192.168.88.1`。

## 6. 检查赛事网段是否与 VF2 冲突

默认 VF2 私网为：

```text
电脑：192.168.88.1/24
MOSS：192.168.88.2/24
```

如果赛事网卡获得 `192.168.88.x/24`，或者 `ip route` 显示赛事网口也拥有覆盖 `192.168.88.0/24` 的直连路由，就存在冲突，必须切换 VF2 私网。若赛事网使用 `10.x.x.x`、`172.x.x.x` 或其他不覆盖 `192.168.88.0/24` 的网段，可以继续使用默认方案。

不要只看地址前三段；应同时看前缀长度和 `ip route`。例如赛事地址为 `192.168.0.10/16` 时也会覆盖 `192.168.88.0/24`。

## 7. 默认无冲突方案

### 7.1 配置 VF2 私网口

确认 `enp3s0` 确实连接 VF2 后执行：

```bash
sudo ip address replace 192.168.88.1/24 dev enp3s0
sudo ip link set dev enp3s0 up
ip -brief address show enp3s0
```

### 7.2 构建无默认网关的 MOSS

比赛时推荐不给 MOSS 设置默认网关：

```bash
cd /home/zjr/Data/moss
make -B APPS_INIT=n SITE_INIT=n \
  IP=192.168.88.2 GW= \
  kernel-vf2
```

`GW=` 为空不会影响 MOSS 与 `192.168.88.1` 通信，但会阻止 MOSS把非本地流量交给电脑转发。

当前 `shell-init` 默认不运行 NTP。只有显式设置 `MOSS_NTP_SYNC=1` 才会启动 `ntpd`，因此赛事外网不可用不会阻塞进入 shell。比赛时不要设置该变量，也不要运行公网 `apk update`、`wget` 或 `curl`。

### 7.3 部署 TFTP 镜像

```bash
sudo install -o tftp -g tftp -m 0644 \
  moss-vf2.bin \
  /srv/tftp/moss-vf2-nowatchdog.bin

sha256sum \
  moss-vf2.bin \
  /srv/tftp/moss-vf2-nowatchdog.bin
```

两行 SHA-256 必须一致。

确认 TFTP 只监听 VF2 私网地址：

```bash
sudo ss -lunp | grep '192.168.88.1:69'
```

如果没有监听，启动：

```bash
sudo /usr/sbin/in.tftpd \
  --listen \
  --user tftp \
  --address 192.168.88.1:69 \
  --secure /srv/tftp
```

不要监听 `0.0.0.0:69`，否则 TFTP 可能同时暴露到赛事网口。

### 7.4 打开串口

```bash
picocom -b 115200 /dev/ttyACM0
```

退出使用 `Ctrl-A`，再按 `Ctrl-X`。

### 7.5 U-Boot 加载并启动 MOSS

进入 `StarFive #` 后逐行执行：

```text
setenv ipaddr 192.168.88.2
setenv serverip 192.168.88.1
setenv netmask 255.255.255.0
printenv ipaddr serverip netmask
ping 192.168.88.1

tftpboot 0x40200000 moss-vf2-nowatchdog.bin
fdt addr 0x42200000
fdt resize 4096
fdt set /chosen bootargs "root=/dev/mmcblk1p3 rw"
fdt print /chosen bootargs
go 0x40200000
```

不要执行 `saveenv`。p2 或 p4 rootfs 只需把 `mmcblk1p3` 改为 `mmcblk1p2` 或 `mmcblk1p4`；p5 是 swap，不能作为 rootfs。

## 8. 验证 MOSS 网络功能

网络受限不会影响 VF2 GMAC、ARP、IPv4、TCP、UDP 和 Socket 功能。推荐在 VF2 私网内完成验收。

### 8.1 检查本地链路

电脑执行：

```bash
ip -brief link show enp3s0
ip -brief address show enp3s0
```

应看到 `UP`、`LOWER_UP` 和 `192.168.88.1/24`。

### 8.2 PC 到 MOSS 的 HTTP/TCP 下载

电脑执行：

```bash
python3 -m http.server 8000 \
  --bind 192.168.88.1 \
  --directory /srv/tftp
```

MOSS 的 Alpine p3/p4 执行：

```sh
wget -O /dev/null \
  http://192.168.88.1:8000/moss-vf2-nowatchdog.bin
```

Debian p2 可使用：

```sh
curl -w '%{http_code} %{size_download}\n' \
  -o /dev/null \
  http://192.168.88.1:8000/moss-vf2-nowatchdog.bin
```

### 8.3 MOSS 向 PC 发送 TCP 数据

电脑执行：

```bash
nc -l 192.168.88.1 19000
```

MOSS 执行：

```sh
echo MOSS_VF2_TX_OK | busybox nc -w 3 192.168.88.1 19000
```

电脑应收到 `MOSS_VF2_TX_OK`。

### 8.4 PC 向 MOSS 发送 TCP 数据

MOSS 执行：

```sh
busybox nc -l -p 19001
```

电脑执行：

```bash
printf 'MOSS_VF2_RX_OK\n' | nc -w 3 192.168.88.2 19001
```

MOSS 应收到 `MOSS_VF2_RX_OK`。

当前 MOSS 不支持 IPv4 raw socket，BusyBox `ping` 在 MOSS 中可能报告 `Socket type not supported`。这不代表 GMAC 或 TCP/IP 失效，应以 HTTP 和双向 TCP 测试为准。U-Boot 自己的 `ping` 可以用于 TFTP 前的链路检查。

## 9. 网段冲突时切换 VF2 私网

本节以候选网段 `192.168.250.0/24` 为例。使用前仍需确认赛事路由没有覆盖它：

```text
电脑：192.168.250.2/24
MOSS：192.168.250.10/24
```

### 9.1 修改电脑 VF2 网口

确认目标是 VF2 私网口后执行：

```bash
sudo ip address flush dev enp3s0 scope global
sudo ip address add 192.168.250.2/24 dev enp3s0
sudo ip link set dev enp3s0 up
ip -4 address show dev enp3s0
ip -4 route
```

不要对赛事网口执行 `flush`。

### 9.2 使用新地址构建 MOSS

```bash
cd /home/zjr/Data/moss
make -B APPS_INIT=n SITE_INIT=n \
  IP=192.168.250.10 GW= \
  kernel-vf2
```

重新部署 `moss-vf2-nowatchdog.bin` 并核对 SHA-256。

### 9.3 修改 TFTP 监听地址

检查 UDP 69 当前由哪个服务或进程占用：

```bash
sudo ss -lunp | grep ':69'
```

停止其中绑定默认地址 `192.168.88.1:69` 的 `in.tftpd` 服务或进程，再启动新实例：

```bash
sudo /usr/sbin/in.tftpd \
  --listen \
  --user tftp \
  --address 192.168.250.2:69 \
  --secure /srv/tftp

sudo ss -lunp | grep '192.168.250.2:69'
```

### 9.4 修改 U-Boot

```text
setenv ipaddr 192.168.250.10
setenv serverip 192.168.250.2
setenv netmask 255.255.255.0
printenv ipaddr serverip netmask
ping 192.168.250.2
tftpboot 0x40200000 moss-vf2-nowatchdog.bin
```

后续 HTTP 和 `nc` 测试也要把电脑地址改成 `192.168.250.2`，MOSS 地址改成 `192.168.250.10`。

## 10. 如果赛题要求 MOSS 访问赛事服务器

两网口隔离方案只允许 MOSS 访问电脑私网，不允许 MOSS 直接访问赛事服务器。这是有意的安全边界。

如果赛题明确要求 MOSS 访问组委会服务器，不要自行开启以下功能：

- `setup_vf2_network.sh install/apply`；
- nftables masquerade；
- Ubuntu 网络共享或热点；
- 两网口 bridge；
- VPN、代理或 SSH 隧道；
- 未经授权的交换机接入。

应向组委会确认：

1. VF2 是否允许作为第二个 MAC/终端接入赛事网络；
2. VF2 应使用 DHCP 还是指定静态地址；
3. 网关、DNS 和允许访问的 IP/端口；
4. 是否允许电脑在 VF2 与赛事网之间路由或桥接；
5. TFTP 是否只能保留在私网。

得到明确授权后再根据组委会方案调整。两根网线本身不会自动把两个网络连通。

## 11. 常见故障

### 11.1 `tftpboot` 超时

检查：

```bash
ip -brief link show enp3s0
ip -brief address show enp3s0
sudo ss -lunp | grep ':69'
ls -l /srv/tftp/moss-vf2-nowatchdog.bin
```

并在 U-Boot 中回读：

```text
printenv ipaddr serverip netmask
ping 192.168.88.1
```

### 11.2 插入赛事网后 TFTP 突然失败

优先检查是否出现网段冲突：

```bash
ip -4 -brief address
ip -4 route
```

确认 `192.168.88.0/24` 只指向 VF2 私网口，TFTP 只监听 `192.168.88.1:69`。

### 11.3 MOSS 能访问电脑但不能访问赛事平台

这是无网关、无 NAT 隔离方案的预期行为，不能据此判定 MOSS 网络驱动失败。先完成 HTTP 和双向 TCP 测试；只有赛题明确要求 MOSS 访问赛事服务器时，才向组委会申请网络配置。

### 11.4 MOSS 中 `ping` 失败

当前 MOSS 不支持 raw socket。使用 `wget`、`curl`、`nc` 或本地测试程序验证 TCP/UDP。

### 11.5 编译时尝试下载工具链或依赖

说明本机缓存不完整。赛前应关闭外网进行一次完整构建演练，并提前准备全部工具链和依赖；现场不能通过非指定网络下载。

## 12. 今晚离线演练清单

在仍可修复问题时完成一次全流程：

1. 移除 VF2 NAT，确认 `ip_forward=0`；
2. 关闭 Wi-Fi；
3. 确认源码、Rust 工具链和依赖均可离线使用；
4. 使用 `IP=192.168.88.2 GW=` 重新构建；
5. 部署并校验 TFTP 文件；
6. 通过 picocom 截停 U-Boot；
7. TFTP 加载 MOSS；
8. 从 p3 或计划使用的 rootfs 进入 shell；
9. 完成 HTTP 下载和双向 `nc` 测试；
10. 执行 `sync` 后安全退出。

## 13. 现场最短检查表

```text
[ ] 两根网线、USB 转网口、USB 串口齐全
[ ] 赛事网口和 VF2 私网口已经通过插拔确认
[ ] Wi-Fi、VPN、代理、热点和移动网络已关闭
[ ] moss-vf2-network.service 已移除或停用
[ ] net.ipv4.ip_forward = 0
[ ] 没有 moss_vf2 NAT 表
[ ] 赛事网段与 VF2 私网不冲突
[ ] VF2 私网口为 192.168.88.1/24（或已成套切换新网段）
[ ] MOSS 使用 IP=192.168.88.2、GW 为空
[ ] TFTP 只监听 VF2 私网地址的 UDP 69
[ ] U-Boot ipaddr/serverip/netmask 与当前私网一致
[ ] 未执行 saveenv
[ ] HTTP 和双向 TCP 测试通过
```

## 14. 比赛结束后恢复平时 NAT

如果比赛结束后需要恢复 VF2 通过电脑访问外部网络，重新打开允许的上联网口，确认默认路由和 VF2 私网口后执行：

```bash
cd /home/zjr/Data/moss
sudo scripts/setup_vf2_network.sh install
sudo scripts/setup_vf2_network.sh status
```

该操作仅用于允许联网的普通开发环境，不要在赛事网络中执行。
