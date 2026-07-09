#!/bin/bash
# Tendril post-boot health check (greenboot "required").
#
# greenboot runs every script in /usr/lib/greenboot/check/required.d/ after each boot. If any exits
# non-zero, greenboot marks the boot a failure; after the boot_counter is exhausted the bootloader
# boots the *previous* bootc deployment instead — so a bad OS update can't brick the appliance. This
# check asserts the two things that make a Tendril host usable: the virtualization stack and the
# control plane.
set -uo pipefail

fail() { echo "greenboot(tendril): $1" >&2; exit 1; }

# The virtualization stack must be up — without libvirt there are no stations.
systemctl is-active --quiet libvirtd || fail "libvirtd is not active"

# The control plane service must be up …
systemctl is-active --quiet tendril-web || fail "tendril-web is not active"

# … and actually answering (HTTPS :443, self-signed). Give it a moment after boot.
for _ in $(seq 1 30); do
  if curl -ksS -o /dev/null --max-time 3 https://127.0.0.1/; then
    echo "greenboot(tendril): healthy"
    exit 0
  fi
  sleep 2
done
fail "tendril-web did not answer on :443"
