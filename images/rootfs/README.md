# QEMU rootfs

四平台标准入口只从本目录读取 QEMU 根文件系统。两份镜像都是无分区表的 ext4 文件系统，QEMU 将其作为唯一 VirtIO 块设备连接，MOSS 直接把该设备挂载为 `/`。

| 平台 | 固定文件 | Alpine | 已安装应用 | SHA-256 |
|---|---|---|---|---|
| QEMU RISC-V64 | `site-rv.img` | 3.22 | BusyBox、iproute2、APK、nginx 1.28.3、Redis 8.0.4、MariaDB 11.4.12 | `3521a53bf363a2ed4ec3f386d4d0d6d4ae20c9dca315888142bb7a4f5f22219a` |
| QEMU LoongArch64 | `site-la.img` | 3.21 | BusyBox、iproute2、APK、nginx 1.26.3、Redis 7.2.11、MariaDB 11.4.12 | `5ca2ceafc9ff29ca829da22ecea7d06c4319a12ab8f919a42e2bc327adcb5ab4` |

镜像文件较大并被 `.gitignore` 排除；本表和根目录 `Makefile` 是正式路径合同。`make site-rv` 和 `make site-la` 始终使用 QEMU 快照模式，不会回写基准镜像。两套 QEMU 都由内核 DHCP 获取 `10.0.2.15/24`，默认网关为 `10.0.2.2`，进入系统后可用 `ip addr add/del` 修改地址。

若替换镜像，必须先执行只读 `e2fsck -fn`，再确认 `ip addr`、APK、nginx、Redis、MariaDB 和 SHA-256，并更新本表。
