#!/usr/bin/env bash
# Build a Tendril vGPU host-driver image variant (see image/vgpu/README.md).
#
#   scripts/build-vgpu-variant.sh amd                 # AMD MxGPU/GIM (fully automated)
#   scripts/build-vgpu-variant.sh nvidia              # NVIDIA vGPU + vgpu_unlock (you supply the .run)
#
# NVIDIA: drop the licensed host driver at image/vgpu/nvidia-vgpu.run, OR set NVIDIA_VGPU_RUN_URL to a
# URL you have legitimate access to (e.g. from your NVIDIA vGPU evaluation). It is NOT fetched from
# unofficial mirrors.
set -euo pipefail

variant="${1:-}"
here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

BASE_TAG="${BASE_TAG:-localhost/tendril:dev}"

build_base_if_missing() {
  if ! podman image exists "$BASE_TAG"; then
    echo "==> Base image $BASE_TAG not found — building it"
    podman build -f image/Containerfile -t "$BASE_TAG" .
  fi
}

case "$variant" in
  amd)
    build_base_if_missing
    echo "==> Building AMD GIM variant"
    podman build -f image/vgpu/Containerfile.amd-gim \
      --build-arg "BASE=$BASE_TAG" -t localhost/tendril:vgpu-amd .
    echo "==> Built localhost/tendril:vgpu-amd  —  deploy: sudo bootc switch localhost/tendril:vgpu-amd && sudo reboot"
    ;;
  nvidia)
    run="image/vgpu/nvidia-vgpu.run"
    if [ ! -f "$run" ]; then
      if [ -n "${NVIDIA_VGPU_RUN_URL:-}" ]; then
        echo "==> Downloading the NVIDIA vGPU driver from NVIDIA_VGPU_RUN_URL"
        curl -fSL -o "$run" "$NVIDIA_VGPU_RUN_URL"
      else
        echo "ERROR: NVIDIA vGPU host driver not found at $run." >&2
        echo "       Get it from NVIDIA's vGPU evaluation/licensing portal (see image/vgpu/README.md)," >&2
        echo "       then: cp NVIDIA-Linux-x86_64-<ver>-vgpu-kvm.run $run   (or set NVIDIA_VGPU_RUN_URL)." >&2
        exit 1
      fi
    fi
    build_base_if_missing
    echo "==> Building NVIDIA vGPU + vgpu_unlock variant"
    podman build -f image/vgpu/Containerfile.nvidia-vgpu-unlock \
      --build-arg "BASE=$BASE_TAG" -t localhost/tendril:vgpu-nvidia .
    echo "==> Built localhost/tendril:vgpu-nvidia  —  deploy: sudo bootc switch localhost/tendril:vgpu-nvidia && sudo reboot"
    ;;
  *)
    echo "usage: $0 {amd|nvidia}" >&2
    exit 2
    ;;
esac
