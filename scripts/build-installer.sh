#!/usr/bin/env bash
#
# Build a bootable Tendril installer from the bootc host image, using bootc-image-builder.
#
#   --type iso  ->  an installer ISO: flash to a USB stick, boot the target, install to its disk
#   --type raw  ->  a raw disk image: `dd` straight onto the target disk (or a USB you boot from)
#
# REQUIREMENTS: a host that can build disk images — podman, loopback devices, and the ability to run
# a privileged container. A plain (unprivileged) LXC container CANNOT do this: it has no
# /dev/loop-control and cannot mount. Run this on bare metal or a full VM.
#
# Usage: scripts/build-installer.sh [--image localhost/tendril:dev] [--type iso|raw] [--output ./out]
set -euo pipefail

IMAGE="localhost/tendril:dev"
TYPE="iso"
OUTPUT="./out"
ROOTFS="xfs"          # root filesystem of the installed system: xfs (default) or btrfs
# BIB installer config (branding + simplified interactive kickstart). Empty ⇒ don't pass one.
CONFIG="image/installer/config.toml"
# Pin bootc-image-builder to a verified-good digest so CI is reproducible and a surprise `:latest`
# push can't break the release build. This is the 2026-05-29 multi-arch index, confirmed to build our
# xfs-rootfs ISO end to end. (Note: the build logs a non-fatal `grub2-probe: failed to get canonical
# path of /dev/mapper/fedora-root` from an RPM scriptlet in the anaconda environment — it's harmless
# and the ISO completes regardless.) Override with $BIB to try a newer builder.
BIB="${BIB:-quay.io/centos-bootc/bootc-image-builder@sha256:7ae88b8d6f2cabfa971d7836b96d6cac19cd1384e658031bd154f9687e929905}"

while [ $# -gt 0 ]; do
  case "$1" in
    --image) IMAGE="$2"; shift 2 ;;
    --type) TYPE="$2"; shift 2 ;;
    --output) OUTPUT="$2"; shift 2 ;;
    --rootfs) ROOTFS="$2"; shift 2 ;;
    --config) CONFIG="$2"; shift 2 ;;
    --no-config) CONFIG=""; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

command -v podman >/dev/null 2>&1 || { echo "podman is required" >&2; exit 1; }
[ -e /dev/loop-control ] || { echo "no loopback devices — run on a host that can build disk images (not an unprivileged LXC)" >&2; exit 1; }
mkdir -p "$OUTPUT"

# Build the host image if it isn't already in local storage.
if ! podman image exists "$IMAGE"; then
  echo "==> Building host image $IMAGE"
  podman build -f image/Containerfile -t "$IMAGE" .
fi

config_mount=()
if [ -n "$CONFIG" ]; then
  [ -f "$CONFIG" ] || { echo "installer config not found: $CONFIG (use --no-config to skip)" >&2; exit 1; }
  # BIB reads /config.toml from inside the container by default.
  config_mount=(-v "$(realpath "$CONFIG")":/config.toml:ro)
  echo "==> Using installer config $CONFIG"
fi

echo "==> Building '$TYPE' installer ($ROOTFS root) from $IMAGE with bootc-image-builder"
sudo podman run --rm --privileged \
  --security-opt label=type:unconfined_t \
  -v "$(realpath "$OUTPUT")":/output \
  -v /var/lib/containers/storage:/var/lib/containers/storage \
  "${config_mount[@]}" \
  "$BIB" --type "$TYPE" --rootfs "$ROOTFS" "$IMAGE"

echo "==> Done. Artifacts:"
find "$OUTPUT" -type f \( -iname '*.iso' -o -iname '*.raw' \) -print

cat <<'NOTE'

Next:
  ISO:  flash to a USB stick (e.g. `dd if=<name>.iso of=/dev/sdX bs=4M status=progress`), boot the
        target machine from it, and follow the installer.
  RAW:  `dd` the image straight onto the target's disk (or a USB you boot the machine from).
NOTE
