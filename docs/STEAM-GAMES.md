# Loading Steam games into stations

Tendril installs the **Steam client** for you (the "Install Steam" wizard toggle on Windows
stations; Bazzite/SteamOS stations boot straight into Steam gaming mode). This guide is about the
**games** — which are large (30–150 GB each) and, by default, re-downloaded on every station. Three
ways to avoid that, best-fit first.

## 1. Golden image with games baked in — recommended

This is the Tendril-native answer, and it turns "load games onto N machines" into "install once,
clone everywhere."

1. **Create one station** (Windows or Bazzite) with a GPU.
2. In the guest, **install Steam, sign in, and install the games** you want on the fleet.
3. Back in Tendril, **capture that station as a golden image** (Media → Station images, or the
   station's *Save as image*).
4. **Clone every other station from that image** when you create them (the wizard's *Base image*
   picker). Clones are copy-on-write — instant, no re-download.
5. **Push it across the fleet** with the golden-image distribute/reimage flow so every node has it.

Best for a curated, fixed set of titles (a LAN party or convention floor). The only cost is image
size — the games live in the image, downloaded/installed exactly once.

> Tie the capture to a Steam account you can keep signed in (or use Steam's offline mode after a
> first online launch), and read the **Licensing** section below before playing on many stations at
> once.

## 2. Shared Steam library on the fleet store

Point every station's Steam **Library Folder** at a shared folder on your Tendril store (the NFS/SMB
share you added under Storage). Games are downloaded once and read by all stations.

- Enable it per-station with the **"Shared Steam library"** wizard toggle (experimental — see below).
- It works best as a **read-mostly, pre-populated** library: update/install from *one* station, play
  from many. Steam does **not** safely support many stations writing/updating the *same* library at
  once (it locks and can corrupt `steamapps` state), so don't run installs/updates from two stations
  into one shared library simultaneously.

Under the hood the toggle **attaches** the host store's `steam-library/` folder to the station over
virtio-fs — it appears inside the guest as the virtio-fs tag **`tendril-steamlib`**. What's automated
today is the *attach* (verified in the domain XML). Mounting it in the guest and adding it as a Steam
library folder is currently a **manual step** (auto-mount + auto-register on first boot is a planned
follow-up):

- **Bazzite/SteamOS:** `sudo mount -t virtiofs tendril-steamlib /mnt/steamlib` (add an fstab line to
  persist), then in Steam → Settings → Storage → *Add Drive* → `/mnt/steamlib`.
- **Windows:** needs the virtio-fs service from the virtio-win tools Tendril installs (WinFsp +
  `VirtioFsSvc`); once running the share shows up as a drive, then Steam → Settings → Storage → *Add
  Drive*. (Windows virtio-fs is finicky — validate before relying on it.)

This whole path is **experimental** and hasn't been validated end-to-end with a real Steam sign-in.
For anything you depend on, use the golden-image approach above.

## 3. Per-station download (baseline)

Each station signs into its own Steam and downloads what it needs. Simplest, but wasteful — N×
bandwidth and N× storage. Fine for one or two stations.

## Licensing — read this before buying hardware 🔑

Steam games are licensed **per account**, and this is the real constraint on a multi-station setup:

- **Family Sharing** lets many machines share one account's library, but **only one person can play
  a given account's games at a time**. You cannot run the same account's copy of a game on ten
  stations simultaneously.
- For **simultaneous** multi-station play you need one of: an account per station, games that
  explicitly allow it, or free-to-play titles.
- A **golden image** (approach 1) copies the *installed files*, not the license — each station still
  authenticates against whatever Steam account signs in there.

Plan the account strategy up front; it's easy to overlook and it dictates how many stations can
actually play a paid title at once.
