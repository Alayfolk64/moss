#!/bin/bash
set -e

BACKUP_FILE="/tmp/resolv.conf.backup.$USER"

if [ ! -f "$BACKUP_FILE" ]; then
    cp /etc/resolv.conf "$BACKUP_FILE"
    echo "已备份原始 DNS 到: $BACKUP_FILE"
else
    echo "检测到已有备份: $BACKUP_FILE"
fi

tee /etc/resolv.conf > /dev/null <<'DNS'
nameserver 100.90.90.90
nameserver 100.90.90.100
DNS

echo "DNS 已修改为:"
cat /etc/resolv.conf
