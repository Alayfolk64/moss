## fcntl Lock Call Flow (Current)

```mermaid
flowchart TD
U["Userspace: fcntl(fd, cmd, arg)"] --> T["Trap to kernel"]
T --> H["handle_syscall(uctx)"]
H --> D{"sysno == fcntl"}
D -->|yes| F["sys_fcntl(fd, cmd, arg)"]
D -->|no| O["other syscall path"]

F --> M{"cmd"}
M -->|F_SETLK| S0["handle_setlk(fd,arg,false)"]
M -->|F_SETLKW| S1["handle_setlk(fd,arg,true)"]
M -->|F_GETLK| G0["handle_getlk(fd,arg)"]
M -->|F_OFD_SETLK/F_OFD_SETLKW| OFD1["currently returns 0 (stub)"]
M -->|F_OFD_GETLK| OFD2["set l_type=F_UNLCK; return 0 (stub)"]
M -->|other fcntl cmds| OTHER["other fcntl branches"]

S0 --> P0["UserPtr<flock64>"]
S1 --> P0
P0 --> V0["validate_lock_type"]
V0 --> F0["File::from_fd + validate_lock_access"]
F0 --> K0["posix_lock_key(dev,ino)"]
K0 --> N0["normalize_range(l_whence,l_start,l_len)"]
N0 --> L0{"loop: conflict?"}
L0 -->|no| A0["apply_owner_lock split/merge/unlock"]
A0 --> R0["return 0"]
L0 -->|yes and nonblocking| E0["return EACCES or EAGAIN"]
L0 -->|yes and blocking| W0["drop map lock"]
W0 --> I0{"pending unblocked signal?"}
I0 -->|yes| EINTR["return EINTR"]
I0 -->|no| Y0["yield_now then retry loop"]
Y0 --> L0

G0 --> P1["UserPtr<flock64>"]
P1 --> V1["validate_lock_type"]
V1 --> U1{"requested type == F_UNLCK?"}
U1 -->|yes| GRET0["set l_type=F_UNLCK; return 0"]
U1 -->|no| F1["File::from_fd + validate_lock_access"]
F1 --> K1["posix_lock_key(dev,ino) + normalize_range"]
K1 --> C1{"find_conflict(owner!=self, overlap, type conflict)"}
C1 -->|found| FILL["set_flock_from_lock: fill type/start/len/pid"]
C1 -->|not found| UNL["set l_type=F_UNLCK, keep user whence/start/len"]
FILL --> GRET1["return 0"]
UNL --> GRET1
```

## POSIX Lock Release Lifecycle

```mermaid
flowchart TD
A["sys_close(fd)"] --> A1["release_posix_locks_for_fd(fd)"]
A1 --> A2["close_file_like(fd)"]

B["sys_close_range(first,last,flags)"] --> B0{"flags has CLOEXEC?"}
B0 -->|yes| B1["only set cloexec bit"]
B0 -->|no| B2["for each fd: release_posix_locks_for_fd(fd) + remove fd"]

C["sys_dup3(old_fd,new_fd,flags)"] --> C1["before overwrite new_fd: release_posix_locks_for_fd(new_fd)"]
C1 --> C2["remove new_fd and install duplicated entry"]

D["do_exit/process exit"] --> D1["release_posix_locks_for_pid(process.pid)"]
```
