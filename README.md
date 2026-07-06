# cosmonaut-installer

A native [libcosmic](https://github.com/pop-os/libcosmic) installer for [COSMIC](https://github.com/pop-os/cosmic-epoch) bootc images. A multi-page wizard drives a privileged Rust install engine over DBus; the engine runs `bootc install to-filesystem` against an opinionated install profile (btrfs + composefs + systemd-boot, optional LUKS). Built for [cosmic-build-meta](https://github.com/RazorfinOS-org/cosmic-build-meta) and projects that junction off it.

## Status

- **Three binaries + one library crate**, built as a single Cargo workspace:
  - `cosmonaut-installer` — libcosmic GUI (runs as the live-env user, no root)
  - `cosmonaut-installer-daemon` — DBus system service (root) hosting the install engine
  - `cosmonaut-installer-cli` — headless driver for CI / scripted installs
  - `cosmonaut-engine` — install orchestrator, linked into the daemon
- **End-to-end install works** in QEMU on the cosmic-build-meta Live ISO: cosmonaut autostarts in the `cosmic-live` session, the wizard collects (image, disk, partitioning mode, encryption), the daemon drives the pipeline, the system reboots into the deployed COSMIC. Erase, free-space, and custom-layout installs plus the LUKS-passphrase unlock chain are all E2E-verified (2026-07-05).
- **Partitioning modes**: erase whole disk (default), install into free space (keeps existing partitions, reuses an existing ESP), and custom per-partition role assignment — the latter two env-gated behind `COSMONAUT_EXPERIMENTAL_LAYOUT=1` until the loopback matrix runs routinely in CI. Non-erase layouts are ESP+root only (bootc's canonical composefs shape); the plan is validated engine-side against a fresh probe before anything is touched.
- **OS detection**: the daemon's `ProbeDisks` ro-mounts candidate partitions to read `os-release` / BLS entry titles / Windows markers, and the GUI names what's on each disk (and what an erase would destroy).
- **Failure UX**: error page with the failing step and log tail, copy/save-logs actions, retry-from-scratch (the engine pre-cleans stale mounts/mappers); all subprocess output is mirrored to the daemon journal.
- **Live-only ship** — depended on by `oci/cosmic-live/stack.bst` and `oci/cosmic-nvidia-live/stack.bst` in cosmic-build-meta; the deployed installed system inherits no cosmonaut files.
- **DBus contract**: well-known name `dev.cosmonaut.Installer1`, object path `/dev/cosmonaut/Installer1`. Methods: `InstallJson(spec_json)` (serde-serialized `cosmonaut_engine::InstallSpec`; blocks until done), `Cancel() -> bool`, `ProbeDisks() -> String` (JSON `Vec<DiskInfo>`). Properties: `State`, `CurrentStep`. Signals: `StepChanged(step, detail)`, `LogLine(stream, line)`, `Progress(percent, step)`, `Completed(success, error)`.

**Known caveats**

- **TPM2-LUKS variants** — the engine implements `tpm2-luks` and `tpm2-luks-passphrase` (via `systemd-cryptenroll --tpm2-device=auto --tpm2-pcrs=7`) and the GUI exposes the radio buttons, but neither path has been E2E tested in QEMU+swtpm. (`luks-passphrase` **is** E2E verified — install + first-boot unlock — as of 2026-07-05; see `docs/install-pipeline.md` §Spike S1.)
- **Cancel only works pre-bootc** — once the engine reaches the `bootc install to-filesystem` step, the GUI Cancel button greys out. There is no clean rollback path from a partially-deployed bootc install.
- **CI needs its first runs** — `.github/workflows/ci.yml` (fmt/clippy/tests + root loopback suite) and `e2e.yml` (nightly QEMU install matrix) exist but haven't run on GitHub yet; e2e also needs cosmic-build-meta to publish a `live-iso` artifact. The E2E driver (`tests/e2e/run-e2e.sh`) is locally verified. The loopback suite (`just test-engine`) needs a local sudo run.
- **Vestigial /boot partition (erase mode only)** — the erase layout still creates a 1 GiB ext4 `/boot` that bootc's composefs backend leaves empty (everything lives on the ESP). Free-space/custom layouts already omit it and boot fine; erase mode drops it once that's soak-tested.
- **skopeo scratch lives on tmpfs** — `/run/cosmonaut/scratch` means the live env needs RAM > compressed image size during the copy. Fine for typical machines; a low-RAM fallback (scratch on the freshly-formatted target) is future work.

## Quick start

**Requirements**

- Rust stable (the workspace pins `rust-toolchain.toml` to `stable`)
- [just](https://github.com/casey/just) for the build recipes
- For local GUI runs: a Wayland session with libcosmic theming (i.e. a COSMIC desktop)
- For an end-to-end install: cosmonaut runs inside a [cosmic-build-meta](https://github.com/RazorfinOS-org/cosmic-build-meta) Live ISO. Build that with `just build-iso` over there.

### Local dev build (host)

```sh
just build           # cargo build --release --workspace; uses host network for crates
```

Binaries land in `target/release/`:
- `cosmonaut-installer` (~25 MB, libcosmic + iced + wgpu)
- `cosmonaut-installer-daemon` (~4 MB)
- `cosmonaut-installer-cli` (~4 MB)

### Eyeball the GUI on the host

```sh
COSMONAUT_IMAGES_JSON=/path/to/images.json \
RUST_LOG=info \
target/release/cosmonaut-installer
```

The catalog override env var lets you point at a sample images.json without `sudo`-installing into `/etc/`. See `crates/cosmonaut-installer/src/images_json.rs` for schema. Clicking through the wizard works on the host; the final `Install` button will fail with a connection error since the system DBus has no `dev.cosmonaut.Installer1` service registered — that's expected (validates the error UI).

### Sandboxed (BST / live ISO) build

cosmic-build-meta's `installer/cosmonaut-installer.bst` element is a `kind: manual` entry with cargo2-vendored sources that runs `just build-release` (which adds `--frozen --offline` for the BST sandbox). See [cosmic-build-meta's installer/](https://github.com/RazorfinOS-org/cosmic-build-meta/tree/main/elements/installer) for the consumer side.

### End-to-end install in QEMU

From the cosmic-build-meta tree:

```sh
just build-iso       # bakes cosmonaut into the cosmic-live filesystem
just boot-iso        # QEMU + KVM + OVMF, virtio /dev/vdb as the install target
```

Cosmonaut autostarts in the cosmic-live session. Click through the wizard; the install takes ~5–10 min for the skopeo pull + `bootc install to-filesystem` + finalize.

## Customising

| Env var | Default | Effect |
|---|---|---|
| `COSMONAUT_IMAGES_JSON` | unset | If set, overrides the catalog path (`/etc/cosmonaut-installer/images.json` and the historical `/etc/bootc-installer/images.json` fallback). Useful for host-side dev runs. |
| `COSMONAUT_BRANDING_JSON` | unset | If set, overrides the branding path (name + progress-page slides). See `crates/cosmonaut-installer/src/branding.rs` for the schema. |
| `COSMONAUT_EXPERIMENTAL_LAYOUT` | unset | `1` shows the free-space and custom partitioning modes on the disk page. Erase mode is always available. |
| `RUST_LOG` | `info` | Tracing filter. `cosmonaut_engine=debug` to see every subprocess `+ command…` line + every stdout/stderr line on the GUI's stderr too. |

The deployed system's hostname is hardcoded to `cosmic` by the install (a first-boot wizard step in cosmic-initial-setup lets the user rename). The image, disk, and encryption knobs are collected in the wizard.

## Project layout

```
cosmonaut-installer/
  Cargo.toml                       Workspace manifest (resolver = "2"). Pins libcosmic
                                   to a specific git rev so cargo2 vendoring shares
                                   one source tree with cosmic-build-meta's other
                                   COSMIC apps.
  Cargo.lock                       Frozen lockfile; cargo2 in the BST sandbox reads
                                   this to populate the cargo2 source list.
  Justfile                         build / build-release / install / check / fmt /
                                   clippy / clean.
  crates/
    cosmonaut-installer/           libcosmic GUI binary
      src/main.rs                  cosmic::app::run + tracing setup
      src/app.rs                   App + Message + Page state machine; wires daemon
                                   stream into Task::stream(...) for live progress
      src/pages/                   welcome, image, disk, encryption, confirm,
                                   progress, done — one file per wizard page
      src/spec.rs                  EncryptionChoice + FinalSpec → daemon wire tuple
      src/images_json.rs           Nested-tree catalog parser + leaves-flatten +
                                   $COSMONAUT_IMAGES_JSON override
      src/disks.rs                 lsblk -ndo NAME,SIZE,MODEL,TYPE wrapper
      src/daemon.rs                zbus proxy for dev.cosmonaut.Installer1; spawns
                                   install on a tokio task, pumps signals into
                                   an UnboundedReceiver
    cosmonaut-installer-daemon/    DBus system service (root)
      src/main.rs                  zbus connection bind + name claim + idle-exit
                                   timer (~30 s after Completed signal)
      src/service.rs               #[interface(name = "dev.cosmonaut.Installer1")]
                                   impl with Install / Cancel + properties + signals
    cosmonaut-installer-cli/       Headless DBus driver
      src/main.rs                  clap args (--image / --disk / --hostname /
                                   --luks-passphrase / --tpm2-luks / --tpm2-luks-passphrase),
                                   subscribes to signals, calls Install
    cosmonaut-engine/              Install orchestrator (library)
      src/lib.rs                   InstallSpec / Encryption / Step / Event types +
                                   install() entry point + best-effort teardown
      src/runner.rs                Subprocess helper streaming stdout/stderr
                                   line-by-line into the event channel
      src/partition.rs             wipefs + sfdisk (3-partition GPT: ESP / boot / root)
      src/format.rs                mkfs.fat ESP + mkfs.ext4 /boot
      src/luks.rs                  cryptsetup luksFormat / luksOpen / luksClose +
                                   systemd-cryptenroll for TPM2 variants
      src/mkfs.rs                  mkfs.btrfs root
      src/mount.rs                 mount {root, /boot, /boot/efi}; idempotent
                                   unmount_all for cleanup
      src/bootc.rs                 skopeo copy <image> oci:… then bootc install
                                   to-filesystem with --composefs-backend
                                   --bootloader systemd --skip-finalize
      src/hostname.rs              echo "cosmic" > /target/etc/hostname
      src/bls.rs                   Inject rd.luks.uuid=<UUID> into BLS
                                   /boot/loader/entries/*.conf
      src/finalize.rs              fstrim + unmount + luksClose
  data/
    dbus/                          DBus system-service activation file + bus policy
    polkit/                        Action definitions (auth_admin_keep defaults)
    systemd/                       systemd unit (Type=dbus, root)
    autostart/                     XDG autostart .desktop (OnlyShowIn=COSMIC)
  docs/install-pipeline.md         Engine spec, distilled from fisherman + verified
                                   against the live ISO. Bookmark this when reading
                                   the engine code.
  rust-toolchain.toml              Pin to stable + rustfmt + clippy
  LICENSE                          GPL-3.0-only
```

## Architecture notes

**Trust boundary.** GUI runs as the unprivileged `cosmic-live` user. It calls the daemon over the system bus; polkit gates `dev.cosmonaut.installer.{install,network}` actions. The daemon (root, DBus-activated) hosts the install engine in-process. Wayland-on-root is avoided entirely; a UI bug can't trash the disk.

**libcosmic.** The GUI uses `cosmic::Application` (the iced-based COSMIC widget set + theme). Pinned to the same libcosmic revision as cosmic-build-meta's other COSMIC apps so cargo2 vendoring in the BST sandbox shares one cached source tree across the whole desktop.

**Engine pipeline.** `cosmonaut_engine::install()` is an `async fn` that runs:

1. **Partition** — `wipefs --all`, then `sfdisk` writes a 3-partition GPT (ESP 512 MiB FAT32, /boot 1 GiB ext4, root remainder). The root partition's GPT type GUID is set to the canonical Linux x86_64 root (`4f68bce3…`) so bootc auto-detection works.
2. **Format** — `mkfs.fat -F32` on ESP, `mkfs.ext4` on /boot.
3. **LUKS** (skipped for `Encryption::None`) — `cryptsetup luksFormat --type luks2` followed by `luksOpen`. TPM2 variants additionally run `systemd-cryptenroll --tpm2-device=auto --tpm2-pcrs=7`; for `tpm2-luks` (no recovery passphrase) the throwaway passphrase used to format the volume is removed via `luksRemoveKey`.
4. **mkfs** — `mkfs.btrfs -f -L cosmic-root` on the root device (or LUKS mapper).
5. **Mount** — root at `/run/cosmonaut/target`, /boot at `/run/cosmonaut/target/boot`, ESP at `/run/cosmonaut/target/boot/efi`.
6. **Bootc** — `skopeo copy <image> oci:/run/cosmonaut/scratch/oci-cache` (composefs-backend requires the raw OCI layout), then `bootc install to-filesystem --composefs-backend --bootloader systemd --skip-finalize --target-imgref <image> --source-imgref oci:… /run/cosmonaut/target`. Cancellable until the subprocess starts; non-cancellable once in flight.
7. **Hostname** — `echo "cosmic" > /run/cosmonaut/target/etc/hostname`. First-boot setup wizard lets the user rename.
8. **BLS** — for LUKS variants, append `rd.luks.uuid=<header UUID>` to the `options` line of every `/boot/loader/entries/*.conf` so initrd unlocks at boot.
9. **Finalize** — `fstrim -v <target>` + recursive unmount + `cryptsetup luksClose`.

Each step emits a typed `Step` event over an mpsc channel; the daemon re-broadcasts as `StepChanged` DBus signals. Subprocess stdout/stderr stream as `Log` events line-by-line.

**Why subprocess instead of library bindings.** Engine shells out to `cryptsetup`, `mkfs.*`, `skopeo`, `bootc`, `sfdisk` etc. instead of binding `libcryptsetup-rs` / `gpt` / etc. The motivation is staying bug-compatible with [tuna-os/fisherman](https://github.com/tuna-os/fisherman) — the install pipeline that the cosmic-build-meta Live ISO previously delegated to via the tuna-installer Flatpak. We can swap individual subprocesses for library bindings in future phases without changing the overall shape.

**Image catalog.** `/etc/cosmonaut-installer/images.json` is a nested tree (preserved schema from the tuna-installer era so downstream cosmic-build-meta junctions can keep their `installer/cosmic-images-json.bst` overrides working). Branches have `children`; leaves have `imgref`. The wizard flattens to a single picker; if the catalog has exactly one leaf, the Image page auto-skips and the user goes Welcome → Disk.

**Reboot.** The Done page calls `org.freedesktop.login1.Manager.Reboot(false)` over the system bus. Auto-fires after a 30 s countdown; manual Reboot/Quit buttons also available.

## Known caveats

| Component / area | Workaround / note |
|---|---|
| skopeo source ref | Bare registry refs (`ngcr.io/foo:bar`) are wrapped in `docker://` before being passed to `skopeo copy`; refs with explicit transport prefixes (`oci:`, `containers-storage:`, etc.) pass through unchanged |
| BLS entries dir | Engine's BLS step edits `*.conf` under the ESP's `loader/entries` (where bootc's composefs backend writes them), falling back to `/boot/loader/entries` for older bootc; if you ever target a grub2 image (don't — we hardcode `--bootloader systemd`) the path may differ |
| Daemon idle-exit timer | 30 s after `Completed` signal; matches the PackageKit one-shot pattern. Cancellable installs (between steps) reset the timer. The DBus service is reactivated cleanly by another `Install()` call. |
| GUI ↔ daemon disconnects | If the daemon dies mid-install, the GUI receives no further signals and never hits Done. There's no retry/reconnect logic yet. |
| Cargo build in BST sandbox | Always `--frozen --offline` (not `--locked`) per cosmic-build-meta's rule for vendored builds |
| libcosmic git rev | Pinned to match cosmic-build-meta's other COSMIC apps so cargo2 caches are shared. Bump in lockstep when cosmic-build-meta upgrades. |
| `lsblk` for disk listing | Shells out to `lsblk -ndo NAME,SIZE,MODEL,TYPE --paths` instead of using UDisks2 over zbus. UDisks2 would give us hot-plug events; lsblk is one subprocess and zero new crates. Phase 2+ upgrade. |

## Credits

- [pop-os/libcosmic](https://github.com/pop-os/libcosmic) — the COSMIC widget set + theme + winit/wgpu integration this GUI is built on.
- [tuna-os/fisherman](https://github.com/tuna-os/fisherman) — the original Go install backend the engine's pipeline shape (and exact `bootc install to-filesystem` flag set) is patterned on. Credit to tuna-os for the recipe.
- [cosmic-build-meta](https://github.com/RazorfinOS-org/cosmic-build-meta) — the bootc/OCI image cosmonaut installs, and the Live ISO it ships inside.

## License

GPL-3.0-only. See `LICENSE`.
