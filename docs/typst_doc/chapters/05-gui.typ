#import "../template.typ": evidence

= 设备与 framebuffer/input 支持

本章替代原模板中的 GUI 章节。当前源码可以确认的是设备节点和 framebuffer/input 兼容接口，而不是 X11、Wayland 或桌面环境服务。

== devfs 设备节点

`kernel/src/pseudofs/dev/mod.rs` 构建 `/dev` 根目录。固定节点包括 `null`、`zero`、`full`、`random`、`urandom`、`rtc0`、`tty`、`tty0`、`console`、`ptmx`、`pts`、`cpu_dma_latency`、`vda` 到 `vdd` 的块设备 stub、`loop0` 到 `loop15` 等。`dev-log` 和 `memtrack` 通过 feature 暴露额外节点。

设备统一实现 `DeviceOps`：支持 `read_at`、`write_at`、`ioctl`、`as_pollable`、`mmap`、`len`、`flags`。VFS 层的 `Device` 同时实现 `FileNodeOps` 和 `Pollable`，因此设备节点可以被普通文件接口、ioctl、poll/epoll 和 mmap 路径使用。

== Framebuffer

`/dev/fb0` 只有在 `axdisplay::has_display()` 为真时加入 devfs。`FrameBuffer::new` 读取 `axdisplay::framebuffer_info()`，保存 framebuffer 虚拟地址和大小，并启动 `fb-refresh` 任务以约 60Hz 调用 `axdisplay::framebuffer_flush()`。

framebuffer 设备支持：

- `read_at` / `write_at` 访问 framebuffer 内存。
- `FBIOGET_VSCREENINFO`、`FBIOPUT_VSCREENINFO`、`FBIOGET_FSCREENINFO`、`FBIOGETCMAP`、`FBIOPUTCMAP` 等 ioctl 的有限实现。
- `mmap` 返回 `DeviceMmap::Physical`，系统调用 mmap 会把它转换为 linear 物理映射。

这说明内核提供 framebuffer 级别的图形内存接口；源码中没有窗口系统、合成器或 X11/Wayland server 的实现。

== Input

输入设备位于 `#[cfg(feature = "input")]` 下。启用 feature 后，`/dev/input` 目录由 `event::input_devices` 构建，`EventDev` 包装 `AxInputDevice`，输出 Linux evdev 风格的 `input_event` 结构。设备支持 `EVIOCGVERSION`、`EVIOCGID`、`EVIOCGRAB` 和若干可变长 `EVIOC*` 读取类 ioctl，并通过 `Pollable` 暴露可读事件。

== TTY 与 PTY

TTY/PTY 代码位于 `kernel/src/pseudofs/dev/tty`。`N_TTY` 被 init 进程绑定为控制终端；`ptmx` 创建 pty master，`pts` 动态暴露 slave。TTY ioctl 覆盖 termios、前台进程组、窗口大小、控制终端设置等路径，和进程组/会话/job-control 共同支撑 shell 与测试程序的终端语义。

== 代码依据

#evidence((
  [`devfs 构建`], [`kernel/src/pseudofs/dev/mod.rs`], [`builder` 添加 null/zero/random/tty/ptmx/loop/fb/input 等节点，部分受 feature 或 display 状态控制。],
  [`设备抽象`], [`kernel/src/pseudofs/device.rs`], [`DeviceOps` 定义 read/write/ioctl/poll/mmap，`Device` 实现 VFS 文件节点。],
  [`framebuffer`], [`kernel/src/pseudofs/dev/fb.rs`], [`FrameBuffer` 支持 framebuffer read/write/ioctl，并通过 `DeviceMmap::Physical` 暴露 mmap。],
  [`input`], [`kernel/src/pseudofs/dev/event.rs`], [`EventDev` 读取 `AxInputDevice` 事件并输出 evdev 风格 `InputEvent`。],
  [`tty/pty`], [`kernel/src/pseudofs/dev/tty/mod.rs`], [`Tty` 支持 termios、foreground pgrp、winsize、ptmx/pts 和 poll。],
))
