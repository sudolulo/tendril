#!/usr/bin/env bash
#
# Verify a media ISO's SHA-256 against the upstream-published checksum, when one exists, and record
# the result next to the file so the console and web UI can show it:
#   <iso>.verified  — sha256 matched the upstream checksum (content: "<hash>  upstream:<url>")
#   <iso>.mismatch  — sha256 did NOT match (content: "<local>  want:<upstream>")
#   <iso>.sha256    — no upstream checksum to compare; local hash recorded for reference
#
# Usage: verify-media.sh /path/to/media.iso
set -euo pipefail

iso="${1:?usage: verify-media.sh <iso>}"
[ -f "$iso" ] || { echo "no such file: $iso" >&2; exit 1; }
name="$(basename "$iso")"

# Where upstream publishes a checksum for each ISO Tendril fetches. Windows (UUP) and virtio-win have
# none — their bytes are verified differently (UUP hashes each component as aria2 downloads it).
url=""
case "$name" in
  bazzite-*.iso)
    variant="${name#bazzite-}"; variant="${variant%.iso}"
    url="https://download.bazzite.gg/bazzite-${variant}-stable-amd64.iso-CHECKSUM"
    ;;
esac

echo "==> Hashing ${name} (this takes a moment for a multi-GB file)"
have="$(sha256sum "$iso" | awk '{print $1}')"
rm -f "$iso.verified" "$iso.mismatch" "$iso.sha256"

if [ -n "$url" ]; then
  want="$(curl -fsSL "$url" 2>/dev/null | awk 'NF{print $1; exit}')" || want=""
  if [ -z "$want" ]; then
    printf '%s\n' "$have" > "$iso.sha256"
    echo "    upstream checksum unavailable; recorded local sha256 ${have}"
  elif [ "$have" = "$want" ]; then
    printf '%s  upstream:%s\n' "$have" "$url" > "$iso.verified"
    echo "    VERIFIED — ${name} matches the published checksum."
  else
    printf '%s  want:%s\n' "$have" "$want" > "$iso.mismatch"
    echo "    !! MISMATCH — ${name} does not match the published checksum." >&2
    echo "       local:    ${have}" >&2
    echo "       upstream: ${want}" >&2
    exit 2
  fi
else
  printf '%s\n' "$have" > "$iso.sha256"
  echo "    no upstream checksum published for ${name}; recorded local sha256 ${have}"
fi
