#!/usr/bin/env bash
# Seed representative data for a Tendril DEMO instance: a few stations (one with a GPU passed
# through, one without — to show the no-GPU caution), fake install media with verification markers,
# and a seat. Safe & idempotent. Run with sudo on the demo host.
#
#   sudo bash scripts/demo-seed.sh
set -euo pipefail
VIRSH="virsh -c qemu:///system"
ISO_DIR=/var/lib/tendril/isos
TENDRIL_ETC=/etc/tendril

# A GPU PCI address to show as "in use" — first passthrough-ish GPU, else a plausible placeholder.
GPU_ADDR="$(tendril-detect 2>/dev/null | grep -oE '[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]' | head -1 || true)"
GPU_ADDR="${GPU_ADDR:-0000:07:00.0}"
read -r dom bus slot func <<<"$(echo "$GPU_ADDR" | sed -E 's/(.{4}):(.{2}):(.{2})\.(.)/0x\1 0x\2 0x\3 0x\4/')"

def() { # name mem_gib vcpus with_gpu
  local name="$1" mem="$2" vcpu="$3" gpu="$4" hostdev=""
  [ "$gpu" = yes ] && hostdev="<hostdev mode='subsystem' type='pci' managed='yes'><source><address domain='$dom' bus='$bus' slot='$slot' function='$func'/></source></hostdev>"
  local f; f="$(mktemp)"
  cat >"$f" <<EOF
<domain type='kvm'>
  <name>$name</name>
  <memory unit='GiB'>$mem</memory>
  <vcpu>$vcpu</vcpu>
  <os><type arch='x86_64' machine='q35'>hvm</type></os>
  <features><acpi/></features>
  <devices><graphics type='vnc' port='-1' listen='0.0.0.0'/>$hostdev</devices>
</domain>
EOF
  $VIRSH define "$f" >/dev/null && echo "  defined $name"
  rm -f "$f"
}

echo "==> stations"
def living-room-windows 8 7 yes
def office-steamos      6 4 no
def den-windows        12 7 yes
$VIRSH start living-room-windows 2>/dev/null && echo "  started living-room-windows" || true

echo "==> install media (fake, with verification markers)"
mkdir -p "$ISO_DIR"
: > "$ISO_DIR/win11.iso";               printf 'a1b2c3d4  local\n' > "$ISO_DIR/win11.iso.sha256"
: > "$ISO_DIR/virtio-win.iso";          printf 'e5f6a7b8  local\n' > "$ISO_DIR/virtio-win.iso.sha256"
: > "$ISO_DIR/bazzite-deck-nvidia.iso"; printf 'deadbeef  upstream:https://bazzite.gg\n' > "$ISO_DIR/bazzite-deck-nvidia.iso.verified"

echo "==> a seat"
mkdir -p "$TENDRIL_ETC"
printf 'Living room\t046d:c52b,045e:028e\n' > "$TENDRIL_ETC/seats.conf"

echo "Demo data seeded. Run the demo web UI with TENDRIL_DEMO=1 (see scripts/demo-setup.sh)."
