#!/usr/bin/env bash
# Update the PINNED public demo (tendril-demo :8202) + its federation peer (tendril-node-b :8091) to the
# current checkout — DELIBERATELY, not on every dev push (deploy-dev.sh handles only the test node).
#
# Run this when you want the demo to reflect newer code — e.g. after cutting a release, checked out at
# the release tag so the demo shows a stable version rather than bleeding-edge dev.
#
#   git checkout v0.21.0 && scripts/deploy-demo.sh    # pin the demo to a release
set -euo pipefail

cd "$(dirname "$0")/.."
SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

echo "==> building tendril-web (release)"
cargo build --release -p tendril-web --locked

echo "==> installing the demo binary to /usr/local/bin/tendril-web-demo"
$SUDO install -m0755 target/release/tendril-web /usr/local/bin/tendril-web-demo

echo "==> restarting the demo services"
$SUDO systemctl restart tendril-demo tendril-node-b
sleep 1
$SUDO systemctl is-active tendril-demo tendril-node-b

echo "==> demo updated to $(git rev-parse --short HEAD 2>/dev/null || echo '(unknown rev)')"
