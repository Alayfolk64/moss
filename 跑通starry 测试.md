```md
# StarryOS 评测样例自动执行改造与复现文档

## 1. 本次代码修改总结

本次修改目标：在评测样例镜像下，系统启动后能自动扫描并执行 `*_testcode.sh`。

### 修改文件

1. `/Users/wuxun/os/StarryOS/src/main.rs`
- 启动入口从：
  - `["/bin/sh", "-c", include_str!("init.sh")]`
- 改为：
  - `["/musl/busybox", "sh", "-c", include_str!("init.sh")]`

原因：
- 评测样例镜像中没有稳定可用的 `/bin/sh`，但有 `/musl/busybox`，可直接进入 shell 执行初始化脚本。

---

2. `/Users/wuxun/os/StarryOS/src/init.sh`
- 新增自动测试执行逻辑：
  - 自动选择 busybox（优先 `/musl/busybox`，其次 `/glibc/busybox`）
  - 扫描 `/musl` 和 `/glibc` 下 `*_testcode.sh`
  - 逐个执行并输出每个脚本的退出码
- 所有工具调用改为显式 busybox（`find/sort/env/sh`），避免 PATH 缺失导致命令找不到。

---

3. `/Users/wuxun/os/StarryOS/Makefile`
- 新增：
  - `all` 目标：生成 `kernel-rv` 和 `kernel-la`
  - `kernel-rv`、`kernel-la` 两个目标
  - `contest-run-rv` 一键评测运行目标
- 默认 QEMU 总线改为 `mmio`（对齐题面命令风格）
- `contest-run-rv` 目前采用评测镜像作为运行盘：
  - `TEST_FS_IMG=$(A)/sdcard-riscv64.img`
  - `DISK_IMG=/__no_disk__.img`（避免误挂本地 rootfs）

---

4. `/Users/wuxun/os/StarryOS/make/Makefile`
- 增加变量：
  - `TEST_FS_IMG ?=`
  - `BOOT_DISK_IMG ?=`
- 用于向 QEMU 传入评测镜像/启动镜像参数。

---

5. `/Users/wuxun/os/StarryOS/make/qemu.mk`
- 扩展 QEMU 磁盘参数逻辑：
  - 支持 `TEST_FS_IMG`
  - 支持 `BOOT_DISK_IMG`（双盘模式预留）
- 当设置 `TEST_FS_IMG` 时，附加测试镜像 drive 参数。

---

## 2. 设计说明（为什么这么改）

原方案想在用户态 `mount` 第二块 ext4 盘再扫描脚本，但当前内核 `sys_mount` 只支持 `tmpfs`，不支持 ext4 挂载，因此用户态挂载不可行。

所以改为：
- 直接让系统从评测样例镜像启动用户态（里面已有 `/musl`、`/glibc`、`*_testcode.sh`、`busybox`）
- 启动后在根目录下直接扫描并执行测试脚本

这个方案已经实测可跑起并进入多个测试组执行。

---

## 3. 同伴完整复现步骤

> 以下步骤在仓库根目录执行：`/Users/wuxun/os/StarryOS`

### Step 0: 确认评测镜像在仓库根目录
确保存在：
- `sdcard-riscv64.img`

---

### Step 1: 启动评测 Docker（宿主机执行）

```bash
docker run -it --rm --privileged \
  --platform linux/amd64 \
  -w /work \
  -v $(pwd):/work \
  docker.io/zhouzhouyi/os-contest:20260104 \
  /bin/bash
```

---

### Step 2: 在容器内设置代理（必须）

> 注意：容器里不要用 `127.0.0.1`，要用 `host.docker.internal`

```bash
export https_proxy=http://host.docker.internal:7897
export http_proxy=http://host.docker.internal:7897
export all_proxy=socks5://host.docker.internal:7897
```

---

### Step 3: 一键启动评测样例自动执行（容器内）

```bash
make contest-run-rv
```

你会看到类似输出：
- `Running on qemu...`
- `Welcome to Starry OS`
- `[TEST] Running /musl/..._testcode.sh`
- 各测试组日志（busybox/basic/iperf/libcbench 等）

---

### Step 4: 生成提交要求内核文件（可选，容器内）

```bash
make all
```

会在仓库根目录生成：
- `kernel-rv`
- `kernel-la`

---

## 4. 常见问题排查

1. **依赖下载失败 / rustup 超时**
- 一般是代理没生效，重新检查 Step 2 三个环境变量。

2. **容器内代理写成 127.0.0.1 导致 Connection refused**
- 改成 `host.docker.internal:7897`。

3. **测试跑很久**
- 部分脚本（如 lmbench/unixbench 类）本身耗时长，属于正常现象。

---

## 5. 当前状态

- 已验证：自动扫描并开始执行多组 `*_testcode.sh`。
- 已具备：同伴可按本文档独立复现运行流程。
```

如果你希望，我可以再给你一版“更短的队友速查版（1页）”用于群里直接贴。