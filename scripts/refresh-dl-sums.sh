#!/usr/bin/env bash
# refresh-dl-sums.sh <dl-dir> — regenerate SHA256SUMS for every ISO served from <dl-dir>.
#
# Shared by release.yml and promote-stable.yml: writing only one release's pair would clobber the
# entries for the stable ISO and every older versioned ISO still linked from release notes, so the
# whole file is rebuilt from what's actually on disk. Also restores the SELinux label nginx serves
# from (a no-op on non-SELinux hosts).
set -euo pipefail

cd "${1:?usage: refresh-dl-sums.sh <dl-dir>}"

: > SHA256SUMS.new
for f in *.iso; do
  [ -e "$f" ] || continue
  # Hash symlinks via their target once; record under every served name.
  h="$(sha256sum "$(readlink -f "$f")" | awk '{print $1}')"
  printf '%s  %s\n' "$h" "$f" >> SHA256SUMS.new
done
mv SHA256SUMS.new SHA256SUMS

chcon -R -t httpd_sys_content_t . 2>/dev/null || true
