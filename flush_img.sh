#!/bin/sh
start_time=$(date +%s)
rm -rf sdcard-la.img
rm -rf sdcard-rv.img
xz -dk sdcard-la.img.xz
xz -dk sdcard-rv.img.xz
end_time=$(date +%s)
echo "Elapsed: $((end_time - start_time))s"