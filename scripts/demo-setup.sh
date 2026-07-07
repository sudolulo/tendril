#!/usr/bin/env bash
# Stand up a public Tendril DEMO behind your reverse proxy: seeds representative data and runs
# tendril-web in read-only demo mode (no login; every action is disabled and returns a banner) as a
# systemd service on 127.0.0.1:$PORT. Point your proxy (e.g. Nginx Proxy Manager) at it and publish
# the URL. Run once:  sudo bash scripts/demo-setup.sh
set -euo pipefail
PORT="${PORT:-8091}"
BIN="${BIN:-/usr/local/bin/tendril-web}"
REPO="${REPO:-$(cd "$(dirname "$0")/.." && pwd)}"

echo "==> seeding demo data"
bash "$REPO/scripts/demo-seed.sh"

echo "==> installing tendril-demo.service on 127.0.0.1:$PORT"
cat > /etc/systemd/system/tendril-demo.service <<EOF
[Unit]
Description=Tendril public demo (read-only, actions disabled)
After=network-online.target libvirtd.service
[Service]
Environment=TENDRIL_DEMO=1
Environment=TENDRIL_WEB_ADDR=127.0.0.1:$PORT
ExecStart=$BIN
Restart=on-failure
[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now tendril-demo.service
sleep 1
echo "==> demo is $(systemctl is-active tendril-demo.service) on 127.0.0.1:$PORT"
echo "    Point your reverse proxy at 127.0.0.1:$PORT, then publish the URL in the README."
