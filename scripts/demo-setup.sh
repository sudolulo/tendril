#!/usr/bin/env bash
# Stand up a permanent Tendril DEMO: seeds representative data and runs tendril-web in read-only demo
# mode (no login; every action is disabled and returns a banner) as a systemd service. Binds all
# interfaces by default so it's reachable across your LAN; put it behind a reverse proxy if you like.
# Run once:  sudo bash scripts/demo-setup.sh   (override the bind with ADDR=127.0.0.1:8091 for proxy-only)
set -euo pipefail
ADDR="${ADDR:-0.0.0.0:8202}"
BIN="${BIN:-/usr/local/bin/tendril-web}"
REPO="${REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
PORT="${ADDR##*:}"

echo "==> seeding demo data"
bash "$REPO/scripts/demo-seed.sh"

echo "==> installing tendril-demo.service on $ADDR"
cat > /etc/systemd/system/tendril-demo.service <<EOF
[Unit]
Description=Tendril public demo (read-only, actions disabled)
After=network-online.target libvirtd.service
[Service]
Environment=TENDRIL_DEMO=1
Environment=TENDRIL_WEB_ADDR=$ADDR
ExecStart=$BIN
Restart=on-failure
[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now tendril-demo.service

# Open the port permanently so LAN clients can reach it.
if systemctl is-active firewalld >/dev/null 2>&1; then
  firewall-cmd --permanent --add-port="${PORT}/tcp" >/dev/null && firewall-cmd --reload >/dev/null
  echo "==> firewalld: ${PORT}/tcp opened (permanent)"
fi

sleep 1
echo "==> demo is $(systemctl is-active tendril-demo.service) on $ADDR"
echo "    LAN:  http://$(hostname -I | awk '{print $1}'):${PORT}/"
