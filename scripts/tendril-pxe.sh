#!/usr/bin/env bash
# Tendril PXE room-provisioner — net-boot a rack of bare-metal machines straight into the *unattended*
# Tendril installer, so a room of PCs images itself hands-off.
#
#   sudo scripts/tendril-pxe.sh --iso /path/to/tendril-<ver>-installer-x86_64.iso [--interface eth0]
#
# It stands up three things on THIS node and leaves them running until you Ctrl-C:
#   • dnsmasq in **proxy-DHCP** mode — it does NOT hand out leases (your existing router keeps doing
#     that); it only *adds* the PXE "here's how to net-boot" options, so it's safe to run on a live LAN.
#   • TFTP — serves the UEFI bootloader (GRUB) + its config.
#   • HTTP — serves the installer kernel/initrd, the ISO as the install source, and the unattended
#     kickstart. Net-booted machines boot with `tendril.unattended`, so they ERASE their disk and
#     install with no prompts (the same touchless path the Unattended ISO entry uses).
#
# UEFI x86_64 targets only (every modern gaming PC). Requires: dnsmasq, a webserver (python3), xorriso
# or bsdtar (to read the ISO), and grub2-efi/shim files (from the ISO). NEEDS VALIDATION on real
# hardware — PXE is firmware-dependent.
set -euo pipefail

ISO="" IFACE="" HTTP_PORT=8080
while [ $# -gt 0 ]; do
  case "$1" in
    --iso) [ $# -ge 2 ] || { echo "--iso needs a value" >&2; exit 2; }; ISO="$2"; shift 2 ;;
    --interface) [ $# -ge 2 ] || { echo "--interface needs a value" >&2; exit 2; }; IFACE="$2"; shift 2 ;;
    --http-port) [ $# -ge 2 ] || { echo "--http-port needs a value" >&2; exit 2; }; HTTP_PORT="$2"; shift 2 ;;
    -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[ -n "$ISO" ] && [ -f "$ISO" ] || { echo "error: --iso <tendril installer ISO> is required" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "error: run as root (binds :67/:69, the PXE ports)" >&2; exit 2; }
command -v dnsmasq >/dev/null || { echo "error: install dnsmasq" >&2; exit 2; }
command -v python3 >/dev/null || { echo "error: python3 needed for the HTTP server" >&2; exit 2; }

# Default to the interface holding the default route.
[ -n "$IFACE" ] || IFACE="$(ip -o route get 1.1.1.1 2>/dev/null | grep -oE 'dev [^ ]+' | awk '{print $2}' | head -1)"
SERVER_IP="$(ip -4 -o addr show dev "$IFACE" 2>/dev/null | grep -oE 'inet [0-9.]+' | awk '{print $2}' | head -1)"
[ -n "$SERVER_IP" ] || { echo "error: couldn't determine this node's IP on $IFACE" >&2; exit 2; }

ROOT="$(mktemp -d /tmp/tendril-pxe.XXXXXX)"
TFTP="$ROOT/tftp"; HTTP="$ROOT/http"
mkdir -p "$TFTP" "$HTTP"
cleanup() { echo; echo "shutting down PXE…"; kill "${DNSMASQ_PID:-}" "${HTTP_PID:-}" 2>/dev/null || true; rm -rf "$ROOT"; }
trap cleanup EXIT INT TERM

echo "==> extracting boot files from the ISO"
extract() { # <iso-path> <dest-dir>
  if command -v xorriso >/dev/null; then
    xorriso -osirrox on -indev "$ISO" -extract / "$2" >/dev/null 2>&1
  elif command -v bsdtar >/dev/null; then
    bsdtar -C "$2" -xf "$ISO"
  else
    echo "error: need xorriso or bsdtar to read the ISO" >&2; exit 2
  fi
}
ISODIR="$ROOT/iso"; mkdir -p "$ISODIR"
extract "$ISO" "$ISODIR"

# Anaconda kernel + initrd (Fedora/bootc ISOs put them under /images/pxeboot).
KERNEL="$(find "$ISODIR" -path '*pxeboot/vmlinuz' -print -quit)"
INITRD="$(find "$ISODIR" -path '*pxeboot/initrd.img' -print -quit)"
[ -n "$KERNEL" ] && [ -n "$INITRD" ] || { echo "error: couldn't find pxeboot vmlinuz/initrd.img in the ISO" >&2; exit 2; }
cp "$KERNEL" "$HTTP/vmlinuz"; cp "$INITRD" "$HTTP/initrd.img"
cp "$ISO" "$HTTP/tendril.iso"   # the install source (inst.repo), served over HTTP

# UEFI bootloader: shim + grub from the ISO's EFI tree.
SHIM="$(find "$ISODIR" -iname 'BOOTX64.EFI' -print -quit)"
GRUB="$(find "$ISODIR" -iname 'grubx64.efi' -print -quit)"
[ -n "$SHIM" ] && [ -n "$GRUB" ] || { echo "error: couldn't find BOOTX64.EFI/grubx64.efi in the ISO" >&2; exit 2; }
cp "$SHIM" "$TFTP/bootx64.efi"; cp "$GRUB" "$TFTP/grubx64.efi"

echo "==> writing the unattended kickstart"
cat >"$HTTP/unattended.ks" <<KS
# Touchless net-install kickstart — mirrors the Unattended ISO entry (ERASES the target disk).
text
network --bootproto=dhcp --activate
# Single-disk safety: only proceed when there's exactly one disk; else Anaconda halts for a human.
%pre --interpreter=/bin/bash
# Only REAL disks: skip zram (Fedora's installer env has a zram swap reporting TYPE=disk) plus
# loop/ram/sr/fd/nbd/dm — same vetted filter as the ISO installer. A single real disk -> touchless;
# 0 or >1 -> leave partitioning unset so Anaconda halts for a human rather than wiping the wrong disk.
disks=""
for d in \$(lsblk -dnro NAME,TYPE | awk '\$2=="disk"{print \$1}'); do
  case "\$d" in zram*|loop*|ram*|sr*|fd*|nbd*|dm-*) continue ;; esac
  [ "\$(cat /sys/block/\$d/removable 2>/dev/null)" = 1 ] && continue
  disks="\$disks \$d"
done
set -- \$disks
if [ "\$#" -eq 1 ]; then
  echo "clearpart --all --initlabel --drives=\$1" > /tmp/part.ks
  echo "part / --fstype=xfs --grow --asprimary --ondisk=\$1" >> /tmp/part.ks
else
  : > /tmp/part.ks
fi
%end
%include /tmp/part.ks
ostreecontainer --url=http://$SERVER_IP:$HTTP_PORT/tendril.iso --no-signature-verification --transport=oci
rootpw --lock
reboot --eject
KS

echo "==> writing the GRUB net-boot menu"
mkdir -p "$TFTP/grub"
cat >"$TFTP/grub/grub.cfg" <<CFG
set timeout=5
menuentry 'Install Tendril — Unattended (ERASES THIS DISK)' {
  linux  (http,$SERVER_IP:$HTTP_PORT)/vmlinuz inst.repo=http://$SERVER_IP:$HTTP_PORT/tendril.iso inst.ks=http://$SERVER_IP:$HTTP_PORT/unattended.ks tendril.unattended inst.text
  initrd (http,$SERVER_IP:$HTTP_PORT)/initrd.img
}
CFG

echo "==> starting HTTP on :$HTTP_PORT"
( cd "$HTTP" && python3 -m http.server "$HTTP_PORT" --bind "$SERVER_IP" ) >/dev/null 2>&1 &
HTTP_PID=$!

echo "==> starting dnsmasq proxy-DHCP + TFTP on $IFACE ($SERVER_IP)"
# proxy-DHCP: the existing router still leases IPs; we only add the boot instructions for UEFI clients.
dnsmasq --no-daemon --keep-in-foreground \
  --interface="$IFACE" --bind-interfaces \
  --dhcp-range="$SERVER_IP,proxy" \
  --dhcp-boot="bootx64.efi" \
  --pxe-service="x86-64_EFI,Tendril net-install,bootx64.efi" \
  --enable-tftp --tftp-root="$TFTP" \
  --log-dhcp &
DNSMASQ_PID=$!

cat <<EOF

════════════════════════════════════════════════════════════════════════════
  Tendril PXE room-provisioner is LIVE on $IFACE ($SERVER_IP)

  Boot the target machines (UEFI, network boot / PXE first) — each will pick up
  the net-boot, ERASE its disk, and install Tendril unattended, then reboot.

  Serving:  http://$SERVER_IP:$HTTP_PORT/  (kernel, initrd, ISO, kickstart)
  Safe on a live LAN: proxy-DHCP adds PXE options only; your router keeps leasing.

  Ctrl-C to stop.
════════════════════════════════════════════════════════════════════════════
EOF
wait
