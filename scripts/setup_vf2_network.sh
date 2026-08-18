#!/bin/sh
set -eu

BOARD_ADDRESS=192.168.88.1
BOARD_NETWORK=192.168.88.0/24
SYSCTL_FILE=/etc/sysctl.d/90-moss-vf2-forward.conf
NFT_FILE=/etc/nftables.d/moss-vf2.nft
CHRONY_FILE=/etc/chrony/conf.d/moss-vf2.conf
SERVICE_FILE=/etc/systemd/system/moss-vf2-network.service
HELPER_FILE=/usr/local/libexec/moss-vf2-network

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "run this command as root" >&2
        exit 1
    fi
}

board_interface() {
    ip -o -4 addr show | awk -v address="$BOARD_ADDRESS" '
        $4 ~ ("^" address "/") { print $2; exit }
    '
}

uplink_interface() {
    ip -4 route show default | awk 'NR == 1 { print $5 }'
}

render_nft() {
    board_if=$1
    uplink_if=$2
    cat <<EOF
table ip moss_vf2 {
    chain forward {
        type filter hook forward priority -10; policy accept;
        iifname "$board_if" oifname "$uplink_if" ip saddr $BOARD_NETWORK accept
        iifname "$uplink_if" oifname "$board_if" ip daddr $BOARD_NETWORK ct state established,related accept
    }

    chain postrouting {
        type nat hook postrouting priority srcnat; policy accept;
        oifname "$uplink_if" ip saddr $BOARD_NETWORK masquerade
    }
}
EOF
}

apply_runtime() {
    board_if=$(board_interface)
    uplink_if=$(uplink_interface)
    if [ -z "$board_if" ] || [ -z "$uplink_if" ]; then
        echo "cannot find VF2 or default uplink interface" >&2
        exit 1
    fi
    sysctl -q -w net.ipv4.ip_forward=1
    nft list table ip moss_vf2 >/dev/null 2>&1 && nft delete table ip moss_vf2
    render_nft "$board_if" "$uplink_if" | nft -f -
}

install_config() {
    require_root
    command -v ip >/dev/null
    command -v nft >/dev/null
    install -d -m 0755 /etc/nftables.d /etc/systemd/system /usr/local/libexec
    install -m 0755 "$0" "$HELPER_FILE"
    printf 'net.ipv4.ip_forward = 1\n' >"$SYSCTL_FILE"
    render_nft "$(board_interface)" "$(uplink_interface)" >"$NFT_FILE"
    cat >"$SERVICE_FILE" <<EOF
[Unit]
Description=MOSS VisionFive 2 host forwarding
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=$HELPER_FILE apply
ExecStop=/usr/sbin/nft delete table ip moss_vf2

[Install]
WantedBy=multi-user.target
EOF
    if command -v chronyc >/dev/null 2>&1 && [ -d /etc/chrony/conf.d ]; then
        printf 'allow %s\n' "$BOARD_NETWORK" >"$CHRONY_FILE"
        systemctl restart chrony 2>/dev/null || systemctl restart chronyd
    fi
    systemctl daemon-reload
    systemctl enable --now moss-vf2-network.service
}

uninstall_config() {
    require_root
    systemctl disable --now moss-vf2-network.service >/dev/null 2>&1 || true
    nft list table ip moss_vf2 >/dev/null 2>&1 && nft delete table ip moss_vf2
    rm -f "$SYSCTL_FILE" "$NFT_FILE" "$CHRONY_FILE" "$SERVICE_FILE" "$HELPER_FILE"
    systemctl daemon-reload
    if command -v chronyc >/dev/null 2>&1; then
        systemctl restart chrony 2>/dev/null || systemctl restart chronyd || true
    fi
}

show_status() {
    printf 'board interface: %s\n' "$(board_interface)"
    printf 'uplink interface: %s\n' "$(uplink_interface)"
    sysctl net.ipv4.ip_forward
    nft list table ip moss_vf2
    if command -v chronyc >/dev/null 2>&1; then
        chronyc tracking || true
    fi
}

case "${1:-}" in
    install) install_config ;;
    apply) require_root; apply_runtime ;;
    status) show_status ;;
    uninstall) uninstall_config ;;
    *) echo "usage: $0 {install|apply|status|uninstall}" >&2; exit 2 ;;
esac
