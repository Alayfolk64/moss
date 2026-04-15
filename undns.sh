#!/bin/bash
set -e

BACKUP_FILE="/tmp/resolv.conf.backup.$USER"

if [ ! -f "$BACKUP_FILE" ]; then
    echo "未找到备份文件: $BACKUP_FILE"
    exit 1
fi

cp "$BACKUP_FILE" /etc/resolv.conf
echo "DNS 已恢复为原来的配置:"
cat /etc/resolv.conf
