# iozone 结果

- 时间: `0623-214609`
- RV: `docs/output_rv.txt`
- LA: `docs/output_la.txt`
- result 取日志里的 `Max throughput per process`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 32.940585 | 34.426766 |
| musl | 34.516144 | 34.558952 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iozone fwrite/fread 4 freaders (kB/sec) | 7123.18 | 29965.08 | 1.762284 | 31828.76 | 1.776203 |
| iozone fwrite/fread 4 fwriters (kB/sec) | 3204.25 | 37171.57 | 1.913798 | 38444.10 | 1.916652 |
| iozone pwrite/pread 4 pread readers (kB/sec) | 14183.61 | 20967.79 | 1.323552 | 29158.99 | 1.513577 |
| iozone pwrite/pread 4 pwrite writers (kB/sec) | 3724.00 | 35489.02 | 1.895066 | 24639.14 | 1.848858 |
| iozone pwritev/preadv 4 initial writers (kB/sec) | 3609.25 | 22719.71 | 1.841140 | 27628.57 | 1.869365 |
| iozone pwritev/preadv 4 rewriters (kB/sec) | 8305.90 | 23685.60 | 1.649327 | 27268.97 | 1.695408 |
| iozone random-read 4 initial writers (kB/sec) | 3401.72 | 29819.45 | 1.885923 | 29058.77 | 1.882937 |
| iozone random-read 4 random readers (kB/sec) | 11082.03 | 21595.18 | 1.486829 | 22068.95 | 1.497845 |
| iozone random-read 4 random writers (kB/sec) | 7140.81 | 5128.39 | 1.000000 | 28142.72 | 1.746264 |
| iozone random-read 4 rewriters (kB/sec) | 7196.32 | 21808.13 | 1.670017 | 23852.87 | 1.698304 |
| iozone read-backwards 4 initial writers (kB/sec) | 3331.39 | 29535.64 | 1.887208 | 32695.96 | 1.898110 |
| iozone read-backwards 4 reverse readers (kB/sec) | 10743.42 | 23505.16 | 1.542934 | 24436.85 | 1.560360 |
| iozone read-backwards 4 rewriters (kB/sec) | 4906.05 | 26548.46 | 1.815204 | 24687.29 | 1.801272 |
| iozone stride-read 4 initial writers (kB/sec) | 3468.09 | 28070.14 | 1.876449 | 26685.30 | 1.870037 |
| iozone stride-read 4 rewriters (kB/sec) | 7004.64 | 25387.36 | 1.724089 | 31782.36 | 1.779606 |
| iozone stride-read 4 stride readers (kB/sec) | 11848.11 | 24216.04 | 1.510733 | 21363.66 | 1.445408 |
| iozone write/read 4 initial writers (kB/sec) | 3524.04 | 34102.63 | 1.896664 | 39906.48 | 1.911693 |
| iozone write/read 4 re-readers (kB/sec) | 13064.75 | 4551.06 | 1.000000 | 22434.02 | 1.417637 |
| iozone write/read 4 readers (kB/sec) | 13135.64 | 31905.22 | 1.588292 | 29068.60 | 1.548116 |
| iozone write/read 4 rewriters (kB/sec) | 7471.07 | 22713.71 | 1.671077 | 29778.64 | 1.749113 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iozone fwrite/fread 4 freaders (kB/sec) | 7123.18 | 22982.31 | 1.690058 | 34972.80 | 1.796322 |
| iozone fwrite/fread 4 fwriters (kB/sec) | 3204.25 | 29536.45 | 1.891515 | 37034.26 | 1.913479 |
| iozone pwrite/pread 4 pread readers (kB/sec) | 14183.61 | 28621.01 | 1.504434 | 24198.25 | 1.413858 |
| iozone pwrite/pread 4 pwrite writers (kB/sec) | 3724.00 | 35116.45 | 1.893953 | 24654.42 | 1.848952 |
| iozone pwritev/preadv 4 initial writers (kB/sec) | 3609.25 | 36651.35 | 1.901525 | 32699.94 | 1.889625 |
| iozone pwritev/preadv 4 rewriters (kB/sec) | 8305.90 | 27598.11 | 1.699041 | 26262.36 | 1.683734 |
| iozone random-read 4 initial writers (kB/sec) | 3401.72 | 36735.37 | 1.907399 | 39914.27 | 1.914774 |
| iozone random-read 4 random readers (kB/sec) | 11082.03 | 23341.14 | 1.525215 | 23404.69 | 1.526504 |
| iozone random-read 4 random writers (kB/sec) | 7140.81 | 23489.99 | 1.696006 | 28238.19 | 1.747122 |
| iozone random-read 4 rewriters (kB/sec) | 7196.32 | 26374.67 | 1.727150 | 32969.53 | 1.781728 |
| iozone read-backwards 4 initial writers (kB/sec) | 3331.39 | 30045.99 | 1.889124 | 36745.93 | 1.909340 |
| iozone read-backwards 4 reverse readers (kB/sec) | 10743.42 | 23931.87 | 1.551083 | 22862.13 | 1.530078 |
| iozone read-backwards 4 rewriters (kB/sec) | 4906.05 | 23810.74 | 1.793956 | 21514.34 | 1.771964 |
| iozone stride-read 4 initial writers (kB/sec) | 3468.09 | 41907.27 | 1.917244 | 37951.20 | 1.908617 |
| iozone stride-read 4 rewriters (kB/sec) | 7004.64 | 31134.14 | 1.775017 | 26691.10 | 1.737566 |
| iozone stride-read 4 stride readers (kB/sec) | 11848.11 | 24633.12 | 1.519017 | 26589.15 | 1.554401 |
| iozone write/read 4 initial writers (kB/sec) | 3524.04 | 32707.26 | 1.892255 | 41649.86 | 1.915389 |
| iozone write/read 4 re-readers (kB/sec) | 13064.75 | 28676.24 | 1.544405 | 25434.65 | 1.486340 |
| iozone write/read 4 readers (kB/sec) | 13135.64 | 24550.47 | 1.464954 | 27174.08 | 1.516611 |
| iozone write/read 4 rewriters (kB/sec) | 7471.07 | 27959.77 | 1.732792 | 25990.60 | 1.712547 |
