# Tendril

**Tendril** is an open-source, Fedora-bootc–based operating system that turns a single multi-GPU
machine (or a cluster of them) into multiple plug-and-play gaming stations — each running **Windows**
or **SteamOS** in a GPU-passthrough VM. It auto-detects your GPUs, installs the right drivers, and
aims to be "can't-break-it" reliable for non-expert users. Think of the DIY Proxmox passthrough
build, but automated and gaming-first.

One machine, many tendrils.

## Status

Early planning. See **[docs/PLAN.md](docs/PLAN.md)** for the full architecture, decision log, and
phased roadmap.

## Highlights

- **Passthrough-first:** N GPUs → N independent gaming stations (the reliable path on consumer hardware).
- **vGPU later:** official (datacenter NVIDIA / AMD MxGPU) and experimental consumer `vgpu_unlock`,
  behind a capability gate, for more stations than physical GPUs.
- **Self-healing host:** Fedora bootc atomic images with greenboot auto-rollback — a bad update can't
  brick the box.
- **Own libvirt orchestrator:** full low-level control of passthrough, vGPU, CPU pinning, Secure Boot
  + TPM (Windows 11), and an optional VM-fingerprint compatibility profile.
- **Controller/agent design:** runs single-node today, scales to a GPU-aware scheduling cluster with
  no rewrite.
- **Streaming-ready:** physical monitors now; Sunshine/Moonlight streaming as a swappable backend.

## AI disclosure

Portions of this project — including design documents and code — were produced with the assistance
of AI tools. All output is reviewed by human maintainers before it lands. See [NOTICE](NOTICE).

## License

TBD — see the open questions in [docs/PLAN.md](docs/PLAN.md).
