# Hardware validation checklist

## The 5-step quick version

1. **Install:** grab <https://dl.onetick.ninja/tendril-latest-installer-x86_64.iso>, write it to a
   USB stick, boot the test box from it, follow the installer. ⚠️ It ERASES the disk you pick.
2. **Log in:** from your laptop, open `https://<box-ip>` (address is shown on the box's monitor),
   accept the cert warning, set a password.
3. **Make a station:** Stations → **+ New station** → pick the second GPU → **Create**. Walk away —
   it downloads and installs Windows by itself (~45 min). PASS = Windows desktop on the monitor,
   nobody touched anything.
4. **Split a GPU (needs the NVIDIA vGPU driver file):** System → vGPU → upload the `.run` → **Build**
   → run the `bootc switch` command it shows → reboot → create **two** stations on the same GPU.
   PASS = both render at once.
5. **Netboot a spare machine (the most valuable test):** Fleet → **Provision a room (PXE)** →
   **Start PXE server** → netboot any spare machine on the LAN. ⚠️ It ERASES that machine, zero
   prompts. PASS = it installs itself and boots into Tendril.

For each step, report **worked / didn't**. If something didn't: **System → Logs → Download**, send
the file plus the step number. Done.

---

## The full checklist

Tendril's code is heavily reviewed, but several paths can only be proven on real hardware — this is
the checklist for doing that. It's written so a friendly tester can run it top-to-bottom on one
machine in an afternoon, no repo checkout needed. Each test says what to do, what PASS looks like,
and what to send back if it fails.

**Ideal test box:** 2 GPUs (one of them an NVIDIA vGPU-capable card), a monitor + USB keyboard/mouse,
a second device on the same LAN for the web UI, and a disk you're happy to ERASE. A spare
second machine (or old laptop) that can netboot unlocks the PXE test.

**Reporting:** for any FAIL, grab **System → Logs → Download** from the web UI (or a photo of the
screen if the UI isn't up) and note the test number. That's all the evidence needed.

> ⚠️ The installer ERASES the disk it targets. Don't run this on a machine with anything you want
> to keep.

---

## Setup (10 min)

1. Download the installer ISO and check it:

   ```
   curl -LO https://dl.onetick.ninja/tendril-latest-installer-x86_64.iso
   curl -LO https://dl.onetick.ninja/SHA256SUMS
   sha256sum -c SHA256SUMS --ignore-missing
   ```

2. Write it to a USB stick (`dd`, Fedora Media Writer, Ventoy — anything).
3. In the test box's firmware: enable **VT-d / AMD-Vi (IOMMU)** and **UEFI boot**. Leave Secure
   Boot on or off — note which.

---

## T1 — Install (guided)

Boot the USB, follow the installer (pick the disk, set language). It reboots into Tendril.

**PASS:** the primary monitor lands in the Tendril console menu; the console shows the web UI URL;
from your laptop, `https://<box-ip>` loads (self-signed cert warning is expected), asks you to set
an admin password, and signs you in.

## T2 — Hardware detection

Web UI → **Hardware**.

**PASS:** both GPUs listed with the right models; the boot/console GPU shows *host-only*; the other
shows *passthrough-ready* with an isolated IOMMU group (or *shared group (ACS override)* — note
which). No IOMMU warning banner at the top.

## T3 — First gaming station (whole-GPU passthrough, unattended Windows)

**Stations → + New station.** Defaults are fine: pick the passthrough GPU, leave *Install
unattended* and *Start now* checked, create. It downloads Windows 11 media (several GB, verified),
then installs completely hands-off — expect 30–60 min total. Watch progress in the station's
console page.

**PASS:** Windows reaches the desktop with no prompts ever answered by a human; the account is the
one from the wizard; the **Guest** panel shows hostname + IP; the monitor attached to that GPU
lights up. Steam/Discord/Sunshine (whichever you left checked) are installed.

**Also validates:** media auto-fetch + checksum verification, the boot-prompt auto-clear, CPU
pinning (check *Low-latency* stayed on), the seed/answer-file generation.

## T4 — Lifecycle + snapshots

On the new station: **Shut down → Start → Force off → Start**. Then take a snapshot, change
something in Windows (make a file on the desktop), **Restore** the snapshot.

**PASS:** every action lands within seconds and the state pill follows; after restore the desktop
file is gone.

## T5 — Golden image + instant clone + data volume

1. Shut the station down → **Save as image** (takes minutes; watch the Media page).
2. Create a second station **from that image** (Base image dropdown), with a **persistent data
   volume** (e.g. 32 GiB) — it should boot straight to the desktop, no install.
3. In Windows, put a file on the data volume (the second disk). Then **Push** the golden image to
   the station from the Media page (reimage).

**PASS:** the clone boots in seconds (no install); after the reimage the OS disk is fresh but the
data-volume file survived.

## T6 — vGPU (the big one — needs your NVIDIA vGPU host `.run` + entitlement)

1. **System → vGPU → NVIDIA → Set up GPU splitting**: upload the
   `NVIDIA-Linux-x86_64-<ver>-vgpu-kvm.run` (or paste a URL you can reach it at).
2. **Build vGPU image** (compiles on the box, several minutes) → then in the console/SSH:
   `sudo bootc switch localhost/tendril:vgpu-nvidia && sudo reboot`.
3. After reboot: Hardware page should show **mdev profiles** on the vGPU card, and the vGPU panel
   should say the guest driver is **automatic** (it fetches the matching Windows/Linux guest
   drivers itself — no upload).
4. Create **two stations on the same GPU**, each picking a vGPU profile (the *recommended* one is
   pre-selected). Unattended Windows install as in T3.
5. When both are up: check the **licensing** status in the vGPU panel (built-in license server) —
   in each guest, `nvidia-smi -q | grep -i license` should show *Licensed*.

**PASS:** two stations render on one physical GPU simultaneously, guest driver installed itself in
both, licensing shows active (no 15-fps throttle after 20 min of use).

## T7 — vGPU re-split (data preserved)

Shut one vGPU station down → its detail page → **GPU split** → pick a different profile → change →
start it.

**PASS:** the station boots into the new split; Windows, games, and files are untouched; no driver
reinstall happens.

## T8 — Driver-branch upgrade (cache invalidation)

Only if you have a second vGPU driver version from a **different branch** (e.g. 17.x and 18.x):
remove the staged driver in the UI, stage the other branch's `.run`, and create a new vGPU station.

**PASS:** the new station gets the **new** branch's guest driver (check the driver version in
Windows), not a stale cached one.

## T9 — OS update + rollback safety

**System → OS image → Check for updates / Update now**, then reboot when one is staged.

**PASS:** the box comes back up, the web UI answers, and `bootc status` shows the new image booted
with the previous one as rollback. (If the box ever fails to boot healthy 3×, it should roll back
by itself — that's greenboot doing its job.)

## T10 — Remote play

Station detail → **Remote play** panel. Install Moonlight on your laptop, add the station's IP,
pair with the PIN.

**PASS:** you get a smooth desktop/game stream on the laptop over LAN.

## T11 — PXE room provisioning (needs a sacrificial second machine)

**Fleet → Provision a room (PXE)** → fetch/pick the installer ISO → **Start PXE server**. Set the
second machine to netboot and power it on. ⚠️ It will ERASE its disk unattended.

**PASS:** the second machine netboots into the installer with zero touches, installs, reboots into
Tendril, and its first login forces a default-password change. Stop the PXE server afterwards and
confirm the panel shows it stopped.

*(This path was rebuilt in 0.23.0 and has never completed on real hardware — it's the single most
valuable test on this list.)*

## T12 — Fleet (only if both boxes are kept)

On the second Tendril box: **Fleet → Join a fleet** with a join code generated on the first.

**PASS:** each box sees the other's stations on its Stations page and can start/stop them; opening
a peer station's console works from either UI.

---

## Results template

```
Box: (CPU / RAM / GPU1 / GPU2 / Secure Boot on|off)
T1 install: PASS/FAIL
T2 detection: PASS/FAIL
T3 unattended windows: PASS/FAIL
T4 lifecycle+snapshots: PASS/FAIL
T5 image+clone+data volume: PASS/FAIL
T6 vGPU split: PASS/FAIL/SKIPPED
T7 re-split: PASS/FAIL/SKIPPED
T8 branch upgrade: PASS/FAIL/SKIPPED
T9 OS update: PASS/FAIL
T10 remote play: PASS/FAIL
T11 PXE: PASS/FAIL/SKIPPED
T12 fleet: PASS/FAIL/SKIPPED
Notes / log downloads attached for any FAIL.
```
