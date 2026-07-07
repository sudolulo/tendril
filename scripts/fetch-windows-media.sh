#!/usr/bin/env bash
#
# Fetch the install media a Tendril Windows station needs, automatically:
#   - a genuine Windows 11 ISO, assembled from Microsoft's Windows Update CDN via UUP dump
#   - the virtio-win driver ISO, so Windows can see the virtio disk during setup
#
# Why UUP dump: Microsoft's consumer Win11 download page is anti-bot gated (it rejects headless /
# datacenter requests). UUP dump instead pulls the genuine component files straight from Microsoft's
# Windows Update servers — which are not gated — and builds the ISO locally. This is the reliable way
# to automate it. You still need a Windows licence to activate the result.
#
# Usage: scripts/fetch-windows-media.sh [--dest DIR] [--edition professional] [--lang en-us]
set -euo pipefail

DEST="/var/lib/tendril/isos"
EDITION="professional"
UUP_LANG="en-us"
while [ $# -gt 0 ]; do
  case "$1" in
    --dest) DEST="$2"; shift 2 ;;
    --edition) EDITION="$2"; shift 2 ;;
    --lang) UUP_LANG="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
mkdir -p "$DEST"

# --- dependency check ---
missing=()
for tool in curl unzip python3 aria2c cabextract wimlib-imagex genisoimage chntpw; do
  command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "Missing tools: ${missing[*]}" >&2
  echo "Debian:  sudo apt install aria2 cabextract wimtools genisoimage chntpw curl unzip python3" >&2
  echo "Fedora:  sudo dnf install aria2 cabextract wimlib-utils genisoimage chntpw curl unzip python3" >&2
  exit 1
fi

# --- virtio-win drivers ---
echo "==> Downloading virtio-win.iso"
curl -fSL --retry 3 -o "$DEST/virtio-win.iso" \
  "https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso"

# --- Windows 11 via UUP dump ---
echo "==> Finding the latest Windows 11 build"
uuid=$(curl -fsSL "https://api.uupdump.net/listid.php?search=Windows%2011&sortByDate=1" | python3 -c "
import sys, json
builds = json.load(sys.stdin)['response']['builds']
vals = builds.values() if isinstance(builds, dict) else builds
# Retail feature-update builds are titled 'Windows 11, version XXHY (...)'. Skip cumulative update
# packages ('.NET ...', 'Preview Update ...') and Insider/prerelease builds, which have no full ISO.
retail = [v for v in vals if v.get('arch') == 'amd64'
          and 'version' in v.get('title', '').lower()
          and 'insider' not in v.get('title', '').lower()]
if not retail:
    sys.exit('no retail Windows 11 build found')
print(retail[0]['uuid'])
")
echo "    build: $uuid"

# Stage on the destination disk, not the default $TMPDIR — /tmp is often tmpfs (RAM-backed) and far
# too small for the multi-GB UUP download.
work="$(mktemp -d "${DEST%/}/uup-work.XXXXXX")"
trap 'rm -rf "$work"' EXIT

echo "==> Fetching the download+convert package"
curl -fsSL -X POST "https://uupdump.net/get.php?id=${uuid}&pack=${UUP_LANG}&edition=${EDITION}" \
  --data "autodl=2&updates=1&cleanup=1" -o "$work/uup.zip"
unzip -q -o "$work/uup.zip" -d "$work"
chmod +x "$work/uup_download_linux.sh"

echo "==> Building the ISO from Microsoft's servers (this downloads several GB and takes a while)"
( cd "$work" && ./uup_download_linux.sh )

iso=$(find "$work" -maxdepth 1 -iname '*.iso' | head -1)
[ -n "$iso" ] || { echo "ISO build failed; artifacts left in $work" >&2; trap - EXIT; exit 1; }
mv "$iso" "$DEST/win11.iso"

# UUP dump verifies each component's hash as aria2 downloads it, so the ISO is assembled from
# verified parts — but there's no single upstream checksum for the finished ISO (it's built locally).
# Record local SHA-256s for reference/display.
"$(dirname "$0")/verify-media.sh" "$DEST/win11.iso" || true
[ -f "$DEST/virtio-win.iso" ] && "$(dirname "$0")/verify-media.sh" "$DEST/virtio-win.iso" || true

echo "==> Done:"
echo "    $DEST/win11.iso     (assembled from hash-verified UUP components)"
echo "    $DEST/virtio-win.iso"
