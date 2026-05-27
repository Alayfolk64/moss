#!/bin/bash

rm -f /Users/wuxun/os/StarryOS/sdcard-rv.img \
      /Users/wuxun/os/StarryOS/sdcard-la.img \
      /Users/wuxun/os/StarryOS/sdcard-rv.img.gz \
      /Users/wuxun/os/StarryOS/sdcard-la.img.gz \
      /Users/wuxun/os/StarryOS/os_serial_out_rv.txt \
      /Users/wuxun/os/StarryOS/os_serial_out_la.txt \
      /Users/wuxun/os/StarryOS/autotest.log \
      /Users/wuxun/os/autotest-for-oskernel/data/console_log

sleep 2

docker run --rm --name starryos-autotest --platform linux/amd64 \
  -v /Users/wuxun/os/StarryOS:/coursegrader/submit \
  -v /Users/wuxun/os/autotest-for-oskernel/data:/coursegrader/testdata \
  -v /Users/wuxun/os/autotest-for-oskernel:/cg \
  -v /Users/wuxun/os/autotest-for-oskernel/data:/mnt/cghook \
  zhouzhouyi/os-contest:20260510 python3 /cg/kernel.zip 2>&1 | tee /Users/wuxun/os/StarryOS/autotest.log