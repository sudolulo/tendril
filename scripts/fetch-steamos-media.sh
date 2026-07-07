#!/usr/bin/env bash
#
# Fetch the install media for a Tendril "SteamOS" gaming station, automatically.
#
# Valve has no generic-PC SteamOS installer as of 2026 — the only official media is the Steam Deck
# recovery image, which is image-based (not scriptable) and AMD-only, so it can't drive an NVIDIA
# passthrough station. Until Valve ships a generic ISO, Tendril's SteamOS station is Bazzite: an
# atomic, Steam-gaming-mode image with an Anaconda ISO that Tendril installs unattended via kickstart.
#
# Usage: scripts/fetch-steamos-media.sh [--dest DIR] [--variant deck-nvidia]
#   Variants: deck-nvidia (default; gaming-mode + NVIDIA), deck-nvidia-open, deck (AMD/Intel),
#             nvidia, nvidia-open, "" (desktop AMD/Intel). See https://bazzite.gg.
set -euo pipefail

DEST="/var/lib/tendril/isos"
VARIANT="deck-nvidia"
while [ $# -gt 0 ]; do
  case "$1" in
    --dest) DEST="$2"; shift 2 ;;
    --variant) VARIANT="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
mkdir -p "$DEST"

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }

# Bazzite publishes ISOs as bazzite-<variant>-stable-amd64.iso (variant omitted for the base image).
if [ -n "$VARIANT" ]; then
  name="bazzite-${VARIANT}-stable-amd64.iso"
else
  name="bazzite-stable-amd64.iso"
fi
url="https://download.bazzite.gg/${name}"
out="$DEST/bazzite-${VARIANT:-base}.iso"

echo "==> Downloading $name (several GB; boots to Steam gaming mode)"
# Download to a hidden .part file and only rename to the final name when complete, so a partial
# download is never listed or usable as a station's install media. -C - resumes if re-run.
tmp="$DEST/.$(basename "$out").part"
curl -fSL -C - --retry 3 -o "$tmp" "$url"
mv -f "$tmp" "$out"

# Verify the download against Bazzite's published SHA-256 (records the result next to the ISO).
"$(dirname "$0")/verify-media.sh" "$out" || {
  echo "==> Checksum verification FAILED — the download may be corrupt. Not using $out." >&2
  exit 1
}

echo "==> Done:"
echo "    $out"
echo "Install a station with:"
echo "    sudo tendril-guest --steamos --create-disk --iso $out --unattend --start"
