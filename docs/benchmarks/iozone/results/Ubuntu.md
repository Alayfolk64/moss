# iozone 结果

- 时间: `0610-165926`
- RV: `/Users/wuxun/os/StarryOS/output_rv.txt`
- LA: `/Users/wuxun/os/StarryOS/output_la.txt`
- result 取日志里的 `Max throughput per process`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 37.869267 | - (缺 20 项) |
| musl | 38.107466 | - (缺 20 项) |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iozone fwrite/fread 4 freaders (kB/sec) | 7123.18 | 93961.62 | 1.924191 | - | - |
| iozone fwrite/fread 4 fwriters (kB/sec) | 3204.25 | 44014.47 | 1.927200 | - | - |
| iozone pwrite/pread 4 pread readers (kB/sec) | 14183.61 | 125270.16 | 1.886776 | - | - |
| iozone pwrite/pread 4 pwrite writers (kB/sec) | 3724.00 | 39334.52 | 1.905325 | - | - |
| iozone pwritev/preadv 4 initial writers (kB/sec) | 3609.25 | 53007.40 | 1.931910 | - | - |
| iozone pwritev/preadv 4 rewriters (kB/sec) | 8305.90 | 64488.22 | 1.871203 | - | - |
| iozone random-read 4 initial writers (kB/sec) | 3401.72 | 50680.59 | 1.932879 | - | - |
| iozone random-read 4 random readers (kB/sec) | 11082.03 | 57174.90 | 1.806173 | - | - |
| iozone random-read 4 random writers (kB/sec) | 7140.81 | 59816.98 | 1.880622 | - | - |
| iozone random-read 4 rewriters (kB/sec) | 7196.32 | 53852.14 | 1.866369 | - | - |
| iozone read-backwards 4 initial writers (kB/sec) | 3331.39 | 54112.34 | 1.938436 | - | - |
| iozone read-backwards 4 reverse readers (kB/sec) | 10743.42 | 67360.77 | 1.840509 | - | - |
| iozone read-backwards 4 rewriters (kB/sec) | 4906.05 | 60051.16 | 1.918302 | - | - |
| iozone stride-read 4 initial writers (kB/sec) | 3468.09 | 53587.41 | 1.935282 | - | - |
| iozone stride-read 4 rewriters (kB/sec) | 7004.64 | 61746.63 | 1.886558 | - | - |
| iozone stride-read 4 stride readers (kB/sec) | 11848.11 | 70127.31 | 1.831049 | - | - |
| iozone write/read 4 initial writers (kB/sec) | 3524.04 | 51921.72 | 1.932128 | - | - |
| iozone write/read 4 re-readers (kB/sec) | 13064.75 | 111125.20 | 1.882432 | - | - |
| iozone write/read 4 readers (kB/sec) | 13135.64 | 104754.04 | 1.874605 | - | - |
| iozone write/read 4 rewriters (kB/sec) | 7471.07 | 72759.75 | 1.897319 | - | - |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iozone fwrite/fread 4 freaders (kB/sec) | 7123.18 | 78221.71 | 1.908936 | - | - |
| iozone fwrite/fread 4 fwriters (kB/sec) | 3204.25 | 56933.28 | 1.943719 | - | - |
| iozone pwrite/pread 4 pread readers (kB/sec) | 14183.61 | 141988.39 | 1.900107 | - | - |
| iozone pwrite/pread 4 pwrite writers (kB/sec) | 3724.00 | 49302.62 | 1.924466 | - | - |
| iozone pwritev/preadv 4 initial writers (kB/sec) | 3609.25 | 51584.00 | 1.930032 | - | - |
| iozone pwritev/preadv 4 rewriters (kB/sec) | 8305.90 | 67844.50 | 1.877574 | - | - |
| iozone random-read 4 initial writers (kB/sec) | 3401.72 | 58544.21 | 1.941895 | - | - |
| iozone random-read 4 random readers (kB/sec) | 11082.03 | 91905.04 | 1.879419 | - | - |
| iozone random-read 4 random writers (kB/sec) | 7140.81 | 57647.62 | 1.876130 | - | - |
| iozone random-read 4 rewriters (kB/sec) | 7196.32 | 83116.91 | 1.913419 | - | - |
| iozone read-backwards 4 initial writers (kB/sec) | 3331.39 | 59243.10 | 1.943767 | - | - |
| iozone read-backwards 4 reverse readers (kB/sec) | 10743.42 | 78860.84 | 1.863767 | - | - |
| iozone read-backwards 4 rewriters (kB/sec) | 4906.05 | 70113.20 | 1.930027 | - | - |
| iozone stride-read 4 initial writers (kB/sec) | 3468.09 | 58394.72 | 1.940610 | - | - |
| iozone stride-read 4 rewriters (kB/sec) | 7004.64 | 73845.40 | 1.905145 | - | - |
| iozone stride-read 4 stride readers (kB/sec) | 11848.11 | 85000.70 | 1.860612 | - | - |
| iozone write/read 4 initial writers (kB/sec) | 3524.04 | 44817.86 | 1.921370 | - | - |
| iozone write/read 4 re-readers (kB/sec) | 13064.75 | 116219.62 | 1.887586 | - | - |
| iozone write/read 4 readers (kB/sec) | 13135.64 | 98518.34 | 1.866668 | - | - |
| iozone write/read 4 rewriters (kB/sec) | 7471.07 | 69316.26 | 1.892218 | - | - |
