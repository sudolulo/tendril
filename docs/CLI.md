# Tendril command-line tools

Everything in the web UI and the `tendril` console runs over the same core as these CLIs. You can
drive Tendril entirely from any of the three.

| Tool | What it does |
|---|---|
| `tendril-web` | Web control plane (Axum + HTMX) — dashboard, create/manage stations, live noVNC console, GPU/media/network |
| `tendril` | Interactive console — a menu over every function below (the OS launches it on the primary display) |
| `tendril-detect` | Enumerates GPUs + IOMMU groups, classifies each as passthrough / host-only |
| `tendril-plan` | Computes the exact `vfio-pci` bind set for a GPU (its whole IOMMU group) |
| `tendril-apply` | Binds a GPU to `vfio-pci` — **dry-run by default**, `--execute` to enact |
| `tendril-domain` | Renders a libvirt domain (Secure Boot + TPM, passthrough hostdevs) for a GPU |
| `tendril-vm` | Renders and (with `--define`) registers a station's VM with libvirt |
| `tendril-guest` | Creates the disk, builds a seed (Windows `autounattend.xml` or SteamOS/Bazzite kickstart), and installs the OS hands-off (`--unattend --start`), then boots from disk (`--finalize`) |
| `tendril-usb` | Lists USB controllers + devices for multi-seat assignment |

Helper scripts:

- `scripts/build-installer.sh` — build the installer ISO (`--rootfs xfs|btrfs`, `--type iso|raw`)
- `scripts/fetch-windows-media.sh` — Windows 11 + virtio-win ISOs (verified as they download)
- `scripts/fetch-steamos-media.sh` — Bazzite gaming-mode ISO (verified against the published checksum)
- `scripts/verify-media.sh` — verify an ISO against its upstream checksum

Try the CLIs without installing the OS — on any Linux host with `/sys`:

```bash
cargo run --bin tendril-detect
```

See **[INSTALL.md](INSTALL.md)** for the full build/deploy/create-a-station walkthrough.
