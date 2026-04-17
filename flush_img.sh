#!/bin/sh
start_time=$(date +%s)
rm -rf sdcard-la.img
rm -rf sdcard-rv.img
cp ./imgs/sdcard-rv.img ./
cp ./imgs/sdcard-la.img ./
end_time=$(date +%s)
echo "Elapsed: $((end_time - start_time))s"