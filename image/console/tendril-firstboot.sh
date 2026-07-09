#!/bin/bash
# Tendril boot-time hardware adaptation.
#
# Runs on every boot (it's cheap) so the appliance always knows its own hardware and surfaces the one
# thing it can't fix for you: IOMMU turned off in firmware. Records a capability snapshot and, when
# IOMMU is inactive, writes a warning the console + web UI display. The passthrough path itself needs
# no per-boot "layer": libvirt binds each assigned GPU to vfio-pci on VM start (managed hostdev), and
# the base image already requests IOMMU + early vfio via kernel args.
set -uo pipefail
STATE=/var/lib/tendril
mkdir -p "$STATE"

# Human-readable capability snapshot (GPUs + per-device passthrough/vGPU class), for the console + support.
/usr/bin/tendril-detect >"$STATE/hardware-report.txt" 2>&1 || true

# IOMMU must be active for any passthrough/vGPU. The kernel only populates iommu_groups when VT-d /
# AMD-Vi is enabled in firmware AND our kargs took effect — an empty dir means it's off in the BIOS.
if ls /sys/kernel/iommu_groups/ 2>/dev/null | grep -q .; then
  rm -f "$STATE/iommu-disabled"
else
  cat >"$STATE/iommu-disabled" <<'EOF'
IOMMU is not active — Tendril can't pass a GPU to a VM until you enable it in the BIOS/UEFI:
  • Intel: enable "VT-d" (plus Virtualization Technology)
  • AMD:   enable "AMD-Vi" / "IOMMU" (plus SVM)
Then reboot. The host kernel already requests it; this is a firmware setting only you can change.
EOF
fi
