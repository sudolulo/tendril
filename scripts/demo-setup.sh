#!/usr/bin/env bash
# Stand up a permanent Tendril DEMO: runs tendril-web in read-only demo mode (no login; every action
# is disabled and returns a banner) as a systemd service. Demo mode shows self-contained canned data
# — it touches no real host state, so it can't collide with a real Tendril instance on the same box.
# Binds all interfaces by default so it's reachable across your LAN; put it behind a reverse proxy if
# you like. Run once:  sudo bash scripts/demo-setup.sh   (override with ADDR=127.0.0.1:8091 for proxy-only)
set -euo pipefail
ADDR="${ADDR:-0.0.0.0:8202}"
BIN="${BIN:-/usr/local/bin/tendril-web}"
PORT="${ADDR##*:}"

echo "==> installing tendril-demo.service on $ADDR"
cat > /etc/systemd/system/tendril-demo.service <<EOF
[Unit]
Description=Tendril public demo (read-only, canned data, actions disabled)
After=network-online.target
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
