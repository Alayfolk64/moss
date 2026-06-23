# iozone 结果

- 时间: `0624-002711`
- RV: `docs/saved_logs/20260624-002017-rv-iozone-cache-retention-pruned/output_rv.txt`
- LA: `docs/saved_logs/20260624-002649-la-iozone-cache-retention-pruned/output_la.txt`
- result 取日志里的 `Max throughput per process`。

## 总分

| libc | RV | LA |
|---|---:|---:|
| glibc | 37.410342 | 37.295328 |
| musl | 37.140767 | 36.504220 |

## glibc

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iozone fwrite/fread 4 freaders (kB/sec) | 7123.18 | 90781.15 | 1.921535 | 88757.69 | 1.919746 |
| iozone fwrite/fread 4 fwriters (kB/sec) | 3204.25 | 27879.04 | 1.885066 | 36159.38 | 1.911385 |
| iozone pwrite/pread 4 pread readers (kB/sec) | 14183.61 | 108348.19 | 1.869092 | 67055.84 | 1.788481 |
| iozone pwrite/pread 4 pwrite writers (kB/sec) | 3724.00 | 28263.17 | 1.868238 | 23276.48 | 1.840010 |
| iozone pwritev/preadv 4 initial writers (kB/sec) | 3609.25 | 26515.51 | 1.863882 | 28019.38 | 1.871187 |
| iozone pwritev/preadv 4 rewriters (kB/sec) | 8305.90 | 70465.42 | 1.882128 | 67985.66 | 1.877829 |
| iozone random-read 4 initial writers (kB/sec) | 3401.72 | 27949.03 | 1.878288 | 29539.02 | 1.884840 |
| iozone random-read 4 random readers (kB/sec) | 11082.03 | 57668.05 | 1.807831 | 51323.14 | 1.784073 |
| iozone random-read 4 random writers (kB/sec) | 7140.81 | 59977.69 | 1.880942 | 38058.47 | 1.812373 |
| iozone random-read 4 rewriters (kB/sec) | 7196.32 | 85476.38 | 1.915809 | 81141.52 | 1.911311 |
| iozone read-backwards 4 initial writers (kB/sec) | 3331.39 | 24864.00 | 1.866016 | 28408.16 | 1.882731 |
| iozone read-backwards 4 reverse readers (kB/sec) | 10743.42 | 58823.55 | 1.817362 | 62591.70 | 1.828357 |
| iozone read-backwards 4 rewriters (kB/sec) | 4906.05 | 78582.65 | 1.937568 | 75056.91 | 1.934636 |
| iozone stride-read 4 initial writers (kB/sec) | 3468.09 | 25307.18 | 1.862960 | 31444.96 | 1.889709 |
| iozone stride-read 4 rewriters (kB/sec) | 7004.64 | 76281.18 | 1.908173 | 85234.84 | 1.917820 |
| iozone stride-read 4 stride readers (kB/sec) | 11848.11 | 48363.25 | 1.755018 | 59296.70 | 1.800189 |
| iozone write/read 4 initial writers (kB/sec) | 3524.04 | 33220.82 | 1.893921 | 24592.87 | 1.856705 |
| iozone write/read 4 re-readers (kB/sec) | 13064.75 | 86669.98 | 1.849259 | 72500.15 | 1.819797 |
| iozone write/read 4 readers (kB/sec) | 13135.64 | 81469.96 | 1.838767 | 87896.70 | 1.850556 |
| iozone write/read 4 rewriters (kB/sec) | 7471.07 | 81638.75 | 1.908486 | 86463.42 | 1.913593 |

## musl

| 项目 | baseline | RV result | RV score | LA result | LA score |
|---|---:|---:|---:|---:|---:|
| iozone fwrite/fread 4 freaders (kB/sec) | 7123.18 | 55423.64 | 1.871478 | 59911.43 | 1.881105 |
| iozone fwrite/fread 4 fwriters (kB/sec) | 3204.25 | 24037.55 | 1.866698 | 4885.08 | 1.344074 |
| iozone pwrite/pread 4 pread readers (kB/sec) | 14183.61 | 88314.91 | 1.839397 | 76525.30 | 1.814655 |
| iozone pwrite/pread 4 pwrite writers (kB/sec) | 3724.00 | 28192.30 | 1.867907 | 28760.85 | 1.870518 |
| iozone pwritev/preadv 4 initial writers (kB/sec) | 3609.25 | 26002.46 | 1.861196 | 30559.03 | 1.881893 |
| iozone pwritev/preadv 4 rewriters (kB/sec) | 8305.90 | 81212.70 | 1.897727 | 53587.55 | 1.845003 |
| iozone random-read 4 initial writers (kB/sec) | 3401.72 | 24157.77 | 1.859187 | 34813.25 | 1.902287 |
| iozone random-read 4 random readers (kB/sec) | 11082.03 | 43813.13 | 1.747061 | 53789.55 | 1.793974 |
| iozone random-read 4 random writers (kB/sec) | 7140.81 | 51343.75 | 1.860922 | 56307.05 | 1.873181 |
| iozone random-read 4 rewriters (kB/sec) | 7196.32 | 73121.73 | 1.901584 | 88003.30 | 1.918227 |
| iozone read-backwards 4 initial writers (kB/sec) | 3331.39 | 27463.99 | 1.878700 | 30097.33 | 1.889313 |
| iozone read-backwards 4 reverse readers (kB/sec) | 10743.42 | 54217.46 | 1.801846 | 41413.80 | 1.740584 |
| iozone read-backwards 4 rewriters (kB/sec) | 4906.05 | 72062.70 | 1.931920 | 62113.03 | 1.921014 |
| iozone stride-read 4 initial writers (kB/sec) | 3468.09 | 28685.86 | 1.879101 | 27561.59 | 1.874169 |
| iozone stride-read 4 rewriters (kB/sec) | 7004.64 | 80694.21 | 1.913195 | 63919.28 | 1.890414 |
| iozone stride-read 4 stride readers (kB/sec) | 11848.11 | 60167.74 | 1.803082 | 29049.57 | 1.592142 |
| iozone write/read 4 initial writers (kB/sec) | 3524.04 | 26407.29 | 1.866550 | 24245.93 | 1.854654 |
| iozone write/read 4 re-readers (kB/sec) | 13064.75 | 73652.14 | 1.822615 | 92436.84 | 1.858663 |
| iozone write/read 4 readers (kB/sec) | 13135.64 | 62733.02 | 1.790610 | 84550.26 | 1.844641 |
| iozone write/read 4 rewriters (kB/sec) | 7471.07 | 62253.82 | 1.879990 | 86580.20 | 1.913709 |
