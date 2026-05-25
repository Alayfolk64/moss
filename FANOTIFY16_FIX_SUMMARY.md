# fanotify16 修复总结

本文档整理本轮围绕 LTP `fanotify16` 的问题分析、修改策略和当前实现状态。

## 提交摘要

本次修改可以概括为：

> 修复 fanotify16 的 rename、ignore 和 bind mount 语义。

具体来说，补充了 `FAN_RENAME` 的 old/new fid name 记录，调整事件匹配、合并与 ignored mark 处理，并让 bind mount 复用源目录身份，以满足 fanotify16 对 fid、路径和事件顺序的检查。

## 1. fanotify16 主要考察什么

`fanotify16` 主要验证带 FID/DFID/name/target fid 的 fanotify 事件语义，重点不是简单看有没有事件，而是检查事件里的每个字段是否符合 Linux 行为：

- `FAN_REPORT_DFID_NAME`：事件应报告父目录 fid 和目录项 name。
- `FAN_REPORT_DIR_FID`：事件应报告父目录 fid，但不报告 name。
- `FAN_REPORT_DIR_FID | FAN_REPORT_FID`：部分 child 事件应额外报告对象自身 fid。
- `FAN_REPORT_DFID_NAME | FAN_REPORT_FID`：父目录 fid + name + child fid。
- `FAN_REPORT_DFID_NAME_TARGET`：create/delete/move 等目录项事件也要带目标对象 fid。
- `FAN_RENAME`：应能区分 old side 的 `OLD_DFID_NAME` 和 new side 的 `NEW_DFID_NAME`。
- `FAN_DELETE_SELF/FAN_MOVE_SELF`：self event 和目录项事件的对象、name、顺序不能混。
- bind mount、rename via temp dir、ignored mark 会进一步放大 fid/path/事件合并的问题。

## 2. 原来的主要问题

原实现更接近“能产生基本 fanotify 事件”的模型，但 `fanotify16` 需要更细的事件结构，因此暴露了这些问题：

- 事件只保存一组 `fid/name`，不能表达“父目录 fid + child fid”。
- `read()` 序列化 event 时只输出一个 fid info record，无法输出第二个 child fid record。
- 缺少 `FAN_EVENT_INFO_TYPE_DFID`、`OLD_DFID_NAME`、`NEW_DFID_NAME` 等 info type。
- 目录项事件以前主要拿父目录路径，导致 name 判断容易变成 `"."` 或丢失真实 basename。
- `unlink/rmdir/rename` 在操作之后再解析路径，删除或移动后可能已经拿不到对象 fid。
- `mknod` 没有发 `FAN_CREATE`，而 `fanotify16` 创建文件用的是 `SAFE_MKNOD()`。
- 事件合并 key 过粗，只按 fid/pid 合并，容易把 open/close、dirent、自身事件、rename 事件错误合并。
- child 的 self event 可能被父目录的 `FAN_EVENT_ON_CHILD` 误收。
- `FAN_RENAME` 没有独立事件，只有 `MOVED_FROM/MOVED_TO`。
- rename 后 close 事件仍可能使用旧路径，导致 close name 不能和 `FILE_NAME2` 对齐。
- ignored mark 对 temp dir 的 `FAN_RENAME/FAN_MOVE` 过滤不够精细。

## 3. 总体修改策略

这轮不是完整复刻 Linux fanotify，而是按 `fanotify16` 的断言做最小范围增强：

- 把事件模型从“单个 fid/name”扩展为“主 info record + 可选第二 info record”。
- dirent 事件统一按“父目录为 primary fid，子对象为 child/object fid”的方式建模。
- 删除、移动这类破坏路径可解析性的 syscall，先 snapshot，再执行真实操作，最后发事件。
- `FAN_RENAME` 独立于 `MOVED_FROM/MOVED_TO`，并保留 old/new 两侧信息。
- 合并事件时把 info type、name、child fid、child name 纳入 key，避免错合并。
- 只在必要位置增加 fanotify hook，不大范围改 VFS 或通用 fs 逻辑。

## 4. fanotify.rs 的核心修改

### 4.1 扩展 event info 类型

新增并使用这些 info type：

- `FANOTIFY_EVENT_INFO_TYPE_DFID`
- `FANOTIFY_EVENT_INFO_TYPE_OLD_DFID_NAME`
- `FANOTIFY_EVENT_INFO_TYPE_NEW_DFID_NAME`

这样 `fanotify16` 在检查 `event_fid->hdr.info_type` 时，可以区分：

- 普通对象 fid：`FID`
- 父目录 fid：`DFID`
- 父目录 fid + name：`DFID_NAME`
- rename old name：`OLD_DFID_NAME`
- rename new name：`NEW_DFID_NAME`

### 4.2 扩展 FanotifyEvent

`FanotifyEvent` 增加了：

- `fid_info_type`
- `child_fid`
- `child_info_type`
- `child_name`

含义是：

- 第一段 info record 表示主要报告对象，通常是父目录 fid 或 self fid。
- 第二段 info record 表示 child fid，或 rename 的 new side dir/name。

### 4.3 扩展 FanotifyPreparedEvent

`FanotifyPreparedEvent` 增加了：

- `child_target`
- `child_info_type`
- `child_name`
- `child_path`
- `object_target`

用途：

- `child_target`：通常表示第二段 info record 的 fid。
- `object_target`：rename 场景中保存真正被移动对象的 fid，避免 new side parent fid 和目标对象 fid 混淆。
- `child_path/child_name`：给 rename new side 或后续 child fid 刷新使用。

### 4.4 支持多段 info record 序列化

`fanotify_event_record()` 改为：

- 先写 metadata。
- 如果有 primary fid，写第一段 fid info。
- 如果有 child fid，再写第二段 info。
- 最后再写 pidfd info。

新增的辅助函数：

- `fanotify_event_info_len()`
- `append_fanotify_event_info()`
- `fanotify_refresh_child_fid()`

这样可以满足 `fanotify16` 对 `num_info`、`info_type`、`info_len`、`fh_len` 的检查。

## 5. 事件匹配和合并策略

### 5.1 区分 dirent/self/child 事件

现在 queue 时会根据事件类型决定报告对象：

- create/delete/move dirent：primary fid 是父目录，name 是目录项名。
- open/close child：primary fid 可以是父目录，第二段是 child fid。
- 非目录 self event：primary fid 是对象自身，不报告 name。
- 目录 self event：name 按测试期望报告为 `"."`。

### 5.2 FAN_RENAME 单独处理

`FAN_RENAME` 不再只是 `MOVED_FROM/MOVED_TO` 的附属效果，而是单独排队：

- old side 使用 `OLD_DFID_NAME`。
- new side 使用 `NEW_DFID_NAME`。
- 需要时保留 object target fid。
- rename 事件不和普通 move/delete/open/close 混合合并。

### 5.3 合并 key 更细

合并事件时不再只看 fid/pid，还会比较：

- primary fid
- primary info type
- primary name
- child fid
- child info type
- child name
- pid
- mask 是否允许合并

新增 `fanotify_can_merge_masks()`，避免以下错误：

- `FAN_RENAME` 和普通 move 事件合并。
- close 和 dirent 事件在不该合并时合并。
- 目录 open/close 和目录 self event 错合并。

### 5.4 插入顺序调整

新增 `fanotify_insert_before_pending_close()`，在 delete/self/rename 等事件遇到 pending close 时，按测试期望把事件插到 close 前面。

这是为了贴近 `fanotify16` 对读出事件顺序的要求。

## 6. syscall hook 的修改

修改点集中在 `kernel/src/syscall/fs/ctl.rs`。

### 6.1 sys_mknodat

补充：

- `notify_fanotify_create(dirfd, &path, false)`

原因：

- `fanotify16` 用 `SAFE_MKNOD(fname1, S_IFREG | 0644, 0)` 创建文件。
- 如果 `mknod` 不发 create，后续第一组 `FAN_CREATE | FAN_MOVED_FROM` 就缺事件。

### 6.2 sys_unlinkat

调整为删除前准备事件：

- `prepare_fanotify_delete_self_event()`
- `prepare_fanotify_delete_event()`

删除完成后再发出 prepared event。

原因：

- 删除后路径可能不可解析。
- `FAN_REPORT_TARGET_FID` 需要删除对象的 child fid。
- 目录 self event 和 dirent delete event 的顺序需要区分。

### 6.3 sys_renameat2

rename 前准备：

- `prepare_fanotify_move_from_event()`
- `prepare_fanotify_move_self_event()`
- `prepare_fanotify_rename_event()`
- `prepare_fanotify_path_alias()`

rename 后：

- commit path alias。
- 按目录/非目录场景调整 `MOVE_SELF` 的发出位置。
- 发出独立 `FAN_RENAME`。
- 发出 `MOVED_FROM/MOVED_TO`。

原因：

- rename 后旧路径失效，所以 `MOVED_FROM`、`MOVE_SELF`、`FAN_RENAME` old side 都必须提前 snapshot。
- close 发生在 rename 后时，需要尽量使用新路径，避免 close 还报告旧 name。

## 7. path alias 机制

新增：

- `FanotifyPathAlias`
- `FANOTIFY_PATH_ALIASES`
- `prepare_fanotify_path_alias()`
- `commit_fanotify_path_alias()`
- `fanotify_path_alias()`
- `forget_fanotify_path_alias()`

目的：

- 文件 rename 后，打开的 fd 可能仍从原 file object 上拿到旧 path。
- `fanotify16` 中 `fname1` rename 到 `fname2` 后再 close，期望 close 对应 `FILE_NAME2`。
- alias 用 target fid 记录新路径，让后续 `path_from_file_like()` 能优先返回 rename 后路径。

删除 self event 后会清理 alias，避免 stale path 长期影响后续事件。

## 8. ignored mark 处理

`fanotify16` 后面有 temp dir 场景：

- 对 `fs_mnt/temp_dir` 添加 ignored mask。
- ignored mask 包含 `FAN_MOVE | FAN_RENAME`。
- 期望经过 temp dir 的 rename 不报告多余 `FAN_RENAME`。
- 但 root 侧仍应保留最终应有的 `MOVED_FROM/MOVED_TO`。

新增处理：

- `mark_ignores_rename()`
- `mark_ignores_child_move()`

策略：

- 对 rename 的 old/new 两侧分别计算 matched/ignored。
- ignored rename 可以匹配 old side 或 new side。
- child move 的 ignore 只过滤经过 ignored dir 的那一侧，避免把 root 侧应保留事件也吞掉。

## 9. bind mount / VFS 相关说明

`fanotify16` 会对 `dname1` 做 bind mount，然后保存 fid、open/close、umount、rename、delete。

因此 fanotify 侧依赖 VFS 的这些语义：

- bind mount 后 fid/device 应接近源目录，而不是生成完全无关的新 `MemoryFs`。
- `absolute_path()` 需要能正确处理 mount root。
- `umount()` 后目录仍要能继续参与 rename/delete 流程。

此前围绕 fanotify16 的调试中，bind mount/path/fid/umount 行为也被纳入修复范围。否则即使 fanotify record 格式正确，也容易出现 unexpected file handle 或 `umount(...)=EBUSY`。

## 10. 当前状态

根据你最新输出，`fanotify16` 的 pass 点已经全部通过。本轮主要补上的能力包括：

- DFID/DFID_NAME/FID/OLD_DFID_NAME/NEW_DFID_NAME record 格式。
- parent fid 和 child fid 的区分。
- `mknod` create 通知。
- delete/rename 前 snapshot。
- rename 独立事件。
- path alias。
- 更细的事件合并和 ignored mark 处理。
- bind mount 复用源目录身份，并修复相关 path/fid/umount 行为。

最终修复点对应三条主线：

- rename：补齐 `FAN_RENAME` 的 old/new side、target fid、事件顺序和合并规则。
- ignore：让 `temp_dir` 上的 ignored `FAN_RENAME/FAN_MOVE` 只过滤经过 ignored 目录的一侧，不误吞 root 侧应保留的 move 事件。
- bind mount：让 bind mount 的 fid/device/path 更接近 Linux 语义，并解决测试中的 unexpected file handle 和 `EBUSY`。

我在本对话中没有替你跑编译和 QEMU；最终结论以你本地/测评机最新输出为准。
