#!/usr/bin/env bash
# Rebuild tendril-web and redeploy the local test + demo instances (this box) — runs on every push to
# dev, so both the test UI and the public demo track every change.
#
# Reusable by hand (as flan — uses sudo) or from CI on the self-hosted `host` runner (as root — no
# sudo needed). It rebuilds the web binary, installs it, and restarts the three co-located services:
#   - tendril-web   (the real test node, :8090, HTTPS)
#   - tendril-demo  (public read-only demo, :8202)
#   - tendril-node-b(federation demo peer, :8091)
set -euo pipefail

cd "$(dirname "$0")/.."
SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

echo "==> building tendril-web (release)"
cargo build --release -p tendril-web --locked

echo "==> installing to /usr/local/bin"
$SUDO install -m0755 target/release/tendril-web /usr/local/bin/tendril-web

echo "==> restarting services"
$SUDO systemctl restart tendril-web tendril-demo tendril-node-b
sleep 1
$SUDO systemctl is-active tendril-web tendril-demo tendril-node-b

echo "==> deployed $(git rev-parse --short HEAD 2>/dev/null || echo '(unknown rev)')"
