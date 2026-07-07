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
FORCE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dest) DEST="$2"; shift 2 ;;
    --edition) EDITION="$2"; shift 2 ;;
    --lang) UUP_LANG="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;   # re-fetch even if a valid copy already exists
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
mkdir -p "$DEST"

# A "valid copy" already on disk = the file exists, is at least a sane minimum size, and hasn't been
# flagged as a checksum mismatch by verify-media.sh. We skip fetching those (unless --force) so a user
# who already has one of win11/virtio only downloads the piece they're missing.
have_valid() {  # $1=path  $2=min_bytes
  [ "$FORCE" -eq 0 ] || return 1
  [ -f "$1" ] || return 1
  [ -f "$1.mismatch" ] && return 1
  [ "$(stat -c%s "$1" 2>/dev/null || echo 0)" -ge "$2" ]
}

# Decide what's actually missing — that drives both what we download and which tools we need.
need_virtio=1; have_valid "$DEST/virtio-win.iso" 104857600  && need_virtio=0   # ~100 MB floor
need_win11=1;  have_valid "$DEST/win11.iso"       3221225472 && need_win11=0   # ~3 GB floor
if [ "$need_virtio" -eq 0 ] && [ "$need_win11" -eq 0 ]; then
  echo "Both win11.iso and virtio-win.iso are already present and valid — nothing to fetch (use --force to re-fetch)."
  exit 0
fi

# --- dependency check (virtio only needs curl; building win11 needs the UUP toolchain) ---
tools=(curl)
[ "$need_win11" -eq 1 ] && tools+=(unzip python3 aria2c cabextract wimlib-imagex genisoimage chntpw)
missing=()
for tool in "${tools[@]}"; do command -v "$tool" >/dev/null 2>&1 || missing+=("$tool"); done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "Missing tools: ${missing[*]}" >&2
  echo "Debian:  sudo apt install aria2 cabextract wimtools genisoimage chntpw curl unzip python3" >&2
  echo "Fedora:  sudo dnf install aria2 cabextract wimlib-utils genisoimage chntpw curl unzip python3" >&2
  exit 1
fi

# --- virtio-win drivers ---
if [ "$need_virtio" -eq 0 ]; then
  echo "==> virtio-win.iso already present and valid — skipping"
else
  echo "==> Downloading virtio-win.iso"
  # Download to a .part file and move it into place only on success, so an interrupted download never
  # leaves a partial virtio-win.iso that the skip check would later mistake for a complete copy.
  # (win11 is already safe this way — it's mv'd in only after a successful build.)
  vtmp="$DEST/.virtio-win.iso.part"
  curl -fSL --retry 3 -C - -o "$vtmp" \
    "https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso"
  mv -f "$vtmp" "$DEST/virtio-win.iso"
fi

# --- Windows 11 via UUP dump ---
if [ "$need_win11" -eq 0 ]; then
  echo "==> win11.iso already present and valid — skipping"
else
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
fi

# UUP dump verifies each component's hash as aria2 downloads it, so the ISO is assembled from
# verified parts — but there's no single upstream checksum for the finished ISO (it's built locally).
# Record local SHA-256s for reference/display. (Runs for whichever files are present.)
[ -f "$DEST/win11.iso" ] && "$(dirname "$0")/verify-media.sh" "$DEST/win11.iso" || true
[ -f "$DEST/virtio-win.iso" ] && "$(dirname "$0")/verify-media.sh" "$DEST/virtio-win.iso" || true

echo "==> Done:"
echo "    $DEST/win11.iso     (assembled from hash-verified UUP components)"
echo "    $DEST/virtio-win.iso"
