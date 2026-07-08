#!/usr/bin/env bash
# Build a vGPU host-driver image variant ON the appliance, from the currently-running bootc image plus
# a driver staged by the web UI. Driven by the Hardware → vGPU panel; also runnable by hand.
#
#   appliance-vgpu-build.sh amd       # AMD GIM (open source; nothing to stage)
#   appliance-vgpu-build.sh nvidia    # NVIDIA vGPU + vgpu_unlock (uses the staged .run)
#
# On success it tags localhost/tendril:vgpu-<variant>; switch into it with:
#   sudo bootc switch localhost/tendril:vgpu-<variant> && sudo reboot
set -euo pipefail

variant="${1:-}"
STAGE="${TENDRIL_VGPU_RUN:-/var/lib/tendril/vgpu/nvidia-vgpu.run}"
CFDIR="${TENDRIL_VGPU_ASSETS:-/usr/libexec/tendril/vgpu}"

[ -d "$CFDIR" ] || { echo "vGPU build assets not found at $CFDIR (need a Tendril image that bakes them)."; exit 1; }

# The image to layer onto = whatever this host is currently booted into.
BASE="$(bootc status --format json 2>/dev/null \
  | python3 -c 'import sys,json
try:
    d=json.load(sys.stdin); print(d["status"]["booted"]["image"]["image"]["image"])
except Exception: pass' 2>/dev/null || true)"
BASE="${BASE#docker://}"
[ -n "$BASE" ] || { echo "Could not determine the running image (is this a bootc host?). Set BASE= manually."; exit 1; }
echo "==> Base image: $BASE"

case "$variant" in
  amd)
    echo "==> Building AMD GIM variant"
    podman build -f "$CFDIR/Containerfile.amd-gim" --build-arg "BASE=$BASE" \
      -t localhost/tendril:vgpu-amd "$CFDIR"
    tag=vgpu-amd
    ;;
  nvidia)
    [ -f "$STAGE" ] || { echo "No NVIDIA .run staged at $STAGE — upload it in the vGPU panel first."; exit 1; }
    echo "==> Building NVIDIA vGPU + vgpu_unlock variant (driver: $STAGE)"
    ctx="$(mktemp -d)"; trap 'rm -rf "$ctx"' EXIT
    mkdir -p "$ctx/image/vgpu"
    cp "$STAGE" "$ctx/image/vgpu/nvidia-vgpu.run"
    cp "$CFDIR/Containerfile.nvidia-vgpu-unlock" "$ctx/Containerfile"
    podman build -f "$ctx/Containerfile" --build-arg "BASE=$BASE" \
      -t localhost/tendril:vgpu-nvidia "$ctx"
    tag=vgpu-nvidia
    ;;
  *)
    echo "usage: $0 {amd|nvidia}" >&2; exit 2 ;;
esac

echo "==> Built localhost/tendril:$tag"
echo "==> Deploy it:  sudo bootc switch localhost/tendril:$tag && sudo reboot"
