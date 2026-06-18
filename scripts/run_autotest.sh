#!/bin/bash
set -e

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TESTDATA_DIR=${TESTDATA_DIR:-/Users/wuxun/os/autotest-for-oskernel/data}
AUTOTEST_DIR=${AUTOTEST_DIR:-/Users/wuxun/os/autotest-for-oskernel}

rm -f "$ROOT_DIR/sdcard-rv.img" \
      "$ROOT_DIR/sdcard-la.img" \
      "$ROOT_DIR/sdcard-rv.img.gz" \
      "$ROOT_DIR/sdcard-la.img.gz" \
      "$ROOT_DIR/os_serial_out_rv.txt" \
      "$ROOT_DIR/os_serial_out_la.txt" \
      "$ROOT_DIR/autotest.log" \
      "$TESTDATA_DIR/console_log"

sleep 2

docker run --rm --name starryos-autotest --platform linux/amd64 \
  -v "$ROOT_DIR:/coursegrader/submit" \
  -v "$TESTDATA_DIR:/coursegrader/testdata" \
  -v "$AUTOTEST_DIR:/cg" \
  -v "$TESTDATA_DIR:/mnt/cghook" \
  zhouzhouyi/os-contest:20260510 python3 /cg/kernel.zip 2>&1 | tee "$ROOT_DIR/autotest.log"
