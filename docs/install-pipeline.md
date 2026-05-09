# Install pipeline (engine spec)

The orchestration our `cosmonaut_engine` crate must implement, distilled
from `tuna-os/fisherman` source and verified against a running COSMIC
live ISO probed on 2026-05-09.

## Steps

The engine runs the following pipeline against an `InstallSpec
{ disk, image, encryption }`. Each step emits a typed `Step` event on
the daemon's DBus signal so the GUI can render progress.

```
1. Partition       — wipefs + GPT layout: ESP (1 GiB FAT32) + root (rest)
2. FormatBoot      — mkfs.fat -F32 on ESP
3. Luks            — cryptsetup luksFormat + luksOpen on root partition (skipped if encryption == None)
4. Mkfs            — mkfs.btrfs on root (or LUKS-opened mapper)
5. Mount           — mount root at /run/cosmonaut/target, mount ESP at /run/cosmonaut/target/boot/efi
6. Bootc           — skopeo copy <image> oci:<scratch>/oci-cache; bootc install to-filesystem (see below)
7. Hostname        — write /run/cosmonaut/target/etc/hostname (default "cosmic")
8. Bls             — for LUKS variants: inject `rd.luks.uuid=<UUID>` into /run/cosmonaut/target/boot/loader/entries/*.conf
9. Finalize        — fstrim -v <target>; umount; cryptsetup luksClose; fsfreeze if needed
```

## Step 6 contract — the `bootc install to-filesystem` invocation

We run on the **live ISO**, never inside a podman container. So fisherman's
`bootcDirect` path applies (no `podman run --privileged` wrapper).

The exact command (matches `BuildBootcArgs` in
`fisherman/internal/install/bootc.go`):

```
skopeo copy <image-ref> oci:/run/cosmonaut/scratch/oci-cache
bootc install to-filesystem \
    --target-imgref <image-ref> \
    --composefs-backend \
    --source-imgref oci:/run/cosmonaut/scratch/oci-cache \
    --bootloader systemd \
    --skip-finalize \
    /run/cosmonaut/target
```

Notes:
- `--composefs-backend` is non-negotiable for our profile; it requires the
  `--source-imgref oci:…` path because composefs needs raw OCI blobs.
- `--target-imgref` is recorded as the upgrade origin so `bootc upgrade`
  pulls from the same ref later.
- `--bootloader systemd` selects systemd-boot; default is `grub2` which we
  don't want.
- `--skip-finalize` because the engine's step 9 does the fstrim/remount/fsfreeze
  itself (fisherman does the same).
- `--unifiedStorage` is **not** emitted as a flag — fisherman's comment:
  "requires bootc to run on bare metal; fisherman always runs bootc inside
  `podman run --privileged`, where bootc builds its internal storage using
  overlay@/run/bootc/storage+/proc/self/fd/3. The fd is not inherited by
  the copy subprocess bootc spawns, so the reference never resolves." We
  bypass the constraint entirely (no podman wrapper), but to stay
  bug-compatible with fisherman we also omit the flag.
- `--disable-selinux` is omitted unless the host kernel has SELinux loaded
  (`/sys/fs/selinux/enforce` exists). FDSDK does not, so we never set it.

## Live-env tool inventory

Verified present on the current `cosmic` Live ISO (boot-iso-headless,
2026-05-09):

| Tool          | Version            | Used by step      |
|---------------|--------------------|-------------------|
| `bootc`       | 1.15.0             | 6                 |
| `skopeo`      | 1.22.0             | 6                 |
| `cryptsetup`  | 2.8.6              | 3, 9              |
| `mkfs.btrfs`  | btrfs-progs 6.19.1 | 4                 |
| `mkfs.fat`    | dosfstools         | 2                 |
| `sfdisk`      | util-linux 2.41.3  | 1 (or use `gpt` crate) |
| `wipefs`      | util-linux 2.41.3  | 1                 |
| `parted`      | 3.6                | (alt for 1)       |
| `podman`      | 5.8.2              | (only if we ever wrap bootc; not in live-direct path) |
| NetworkManager (nmcli) | active     | (Phase 2a fallback if iwd is absent) |

All required `bootc install to-filesystem` flags are present in 1.15.0:
`--composefs-backend`, `--bootloader`, `--source-imgref`, `--target-imgref`,
`--skip-finalize`, `--disable-selinux`, `--root-mount-spec`,
`--boot-mount-spec`.

## Gaps to close before Phase 3 (live-ISO swap)

These have to be added to the live-env filesystem so the engine has what
it needs at install time:

| Tool       | Why                                                              | How                                                  |
|------------|------------------------------------------------------------------|------------------------------------------------------|
| `sgdisk`   | Optional; we can use `sfdisk` or the Rust `gpt` crate instead    | If we want it: depend on `freedesktop-sdk.bst:components/gptfdisk.bst` |
| `iwd`      | Wifi page (Phase 2a) talks to it via `net.connman.iwd` DBus      | Add to `live-extras.bst` (or `cosmonaut-installer.bst` runtime deps) |

The `cosmonaut-installer.bst` element should declare runtime deps on the
"used by step" tools above so the live filesystem is guaranteed to have
them after we drop the tuna-installer chain that currently pulls them in.

## Image-source story

The current `cosmic` ISO does **not** bake the bootc image into
`/var/lib/containers/storage/` — `podman images` is empty on first boot.
That means today's install requires network to pull
`ngcr.io/razorfinos/cosmic:nightly` from the registry before
`bootc install` runs.

This is fine for Phase 1–4 (network-required is the assumed default;
offline ISO is deferred per plan §Risks #8). When we revisit offline,
we either:
- bake the image as an `oci-archive` into `/usr/lib/cosmonaut-installer/images/`
  and have the engine pass `oci-archive:<path>` as the image ref, or
- pre-load it into containers-storage at first live boot.

## What this de-risks

- The engine's headline step (`bootc install`) is **just a subprocess**, no
  podman wrapper, no privileged container. Trivial to express in Rust.
- All flag names match what fisherman uses, against the bootc version we
  actually ship. No surprise renames between bootc versions to worry about
  when we pin our cosmonaut release.
- The live env has every required tool; cosmonaut just needs to declare
  them as runtime deps to guarantee the chain after we delete tuna-installer.
- The engine can use `bootc status --json` to introspect bootc state for
  diagnostics (returns valid JSON, verified).

## What still needs in-VM verification (Phase 1+ work, not blocking)

- The full step pipeline against a real disk, end-to-end, with reboot
  into the deployed system. Best done while developing the engine.
- LUKS path: confirm `rd.luks.uuid=` injection into BLS entries actually
  unlocks at first boot.
- Cancel semantics: what does a SIGTERM mid-`bootc install` leave behind?
