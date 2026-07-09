#!/usr/bin/env bash
# Rebuild tendril-web and redeploy ONLY the local test node (this box) — runs on every push to dev, so
# the test UI tracks every change.
#
# The public demo (tendril-demo :8202) + its federation peer (tendril-node-b :8091) are deliberately
# NOT touched here: they run a separate pinned binary (/usr/local/bin/tendril-web-demo) so the demo
# stays stable and doesn't show half-finished dev features. Update the demo on purpose with
# scripts/deploy-demo.sh.
#
# Reusable by hand (as flan — uses sudo) or from CI on the self-hosted `host` runner (as root).
set -euo pipefail

cd "$(dirname "$0")/.."
SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

echo "==> building tendril-web (release)"
cargo build --release -p tendril-web --locked

echo "==> installing the test-node binary to /usr/local/bin"
$SUDO install -m0755 target/release/tendril-web /usr/local/bin/tendril-web

echo "==> restarting the test node"
$SUDO systemctl restart tendril-web
sleep 1
$SUDO systemctl is-active tendril-web

echo "==> deployed $(git rev-parse --short HEAD 2>/dev/null || echo '(unknown rev)') to the test node (demo left pinned)"
