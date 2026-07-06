//! Root-side disk probe: the engine's lsblk/sfdisk scan plus OS
//! detection via short-lived read-only mounts. Backs the `ProbeDisks`
//! DBus method; the GUI renders the results as "what's on this disk"
//! chips so users can pick the right target confidently.
//!
//! Detection rules:
//! - Linux: ext4/btrfs/xfs partition, unmounted → ro-mount, read
//!   `etc/os-release` (or `usr/lib/os-release`); btrfs also tries the
//!   common `@` and `root` subvolumes. `PRETTY_NAME` wins.
//! - Windows: an ESP containing `EFI/Microsoft/Boot/bootmgfw.efi` is
//!   labeled "Windows Boot Manager"; an ntfs partition is labeled
//!   "Windows" via ntfs3 ro-mount (`Windows/` dir present) or, failing
//!   the mount, by filesystem type alone.
//!
//! Every partition probe is capped at 5 s; failures degrade to "no OS
//! detected", never to an error.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cosmonaut_engine::probe::{self, DetectedOs, DiskInfo, OsKind};

const PROBE_MOUNT_ROOT: &str = "/run/cosmonaut/probe";
const PER_PARTITION_TIMEOUT: Duration = Duration::from_secs(5);
const GPT_ESP: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

/// Full probe: engine scan + OS detection.
pub async fn probe_disks_with_os() -> anyhow::Result<Vec<DiskInfo>> {
    let mut disks = tokio::task::spawn_blocking(probe::probe_disks_blocking).await??;

    for disk in &mut disks {
        for part in &mut disk.partitions {
            if part.mounted {
                // Live medium / already in use — don't touch.
                continue;
            }
            let path = part.path.clone();
            let fstype = part.fstype.clone();
            let is_esp = part.part_type.as_deref() == Some(GPT_ESP);
            let result = tokio::time::timeout(
                PER_PARTITION_TIMEOUT,
                tokio::task::spawn_blocking(move || detect_os(&path, fstype.as_deref(), is_esp)),
            )
            .await;
            part.detected_os = match result {
                Ok(Ok(os)) => os,
                Ok(Err(e)) => {
                    tracing::warn!(part = %part.path.display(), error = %e, "os probe panicked");
                    None
                }
                Err(_) => {
                    tracing::warn!(part = %part.path.display(), "os probe timed out");
                    None
                }
            };
        }
    }
    Ok(disks)
}

/// Blocking single-partition detection. Never errors — a partition we
/// can't read simply has no detected OS.
fn detect_os(device: &Path, fstype: Option<&str>, is_esp: bool) -> Option<DetectedOs> {
    match fstype {
        Some("vfat") if is_esp => with_ro_mount(device, "vfat", &[], probe_esp).flatten(),
        Some("ext4") | Some("xfs") => {
            with_ro_mount(device, fstype.unwrap(), &[], read_os_release).flatten()
        }
        Some("btrfs") => {
            // Top level first, then the distro-conventional subvolumes.
            for opts in [&[][..], &["subvol=@"][..], &["subvol=root"][..]] {
                if let Some(Some(os)) = with_ro_mount(device, "btrfs", opts, read_os_release) {
                    return Some(os);
                }
            }
            // bootc-composefs roots have no os-release at the top level
            // (just composefs/ + state/); the human-readable name lives
            // in the ESP's BLS entries. Mark the root generically.
            with_ro_mount(device, "btrfs", &[], |root| {
                (root.join("composefs").is_dir() && root.join("state").is_dir()).then(|| {
                    DetectedOs {
                        pretty_name: "Linux (bootc)".into(),
                        kind: OsKind::Linux,
                    }
                })
            })
            .flatten()
        }
        Some("ntfs") => {
            // ntfs3 in-kernel driver; fall back to type-based labeling.
            let mounted = with_ro_mount(device, "ntfs3", &[], |root| {
                root.join("Windows").is_dir().then(|| DetectedOs {
                    pretty_name: "Windows".into(),
                    kind: OsKind::Windows,
                })
            });
            match mounted {
                Some(found) => found,
                None => Some(DetectedOs {
                    pretty_name: "Windows (NTFS)".into(),
                    kind: OsKind::Windows,
                }),
            }
        }
        _ => None,
    }
}

/// Mount `device` read-only at a private dir, run `f`, unmount.
/// Returns None when the mount itself fails.
fn with_ro_mount<T>(
    device: &Path,
    fstype: &str,
    extra_opts: &[&str],
    f: impl FnOnce(&Path) -> T,
) -> Option<T> {
    use std::process::Command;

    let dir = PathBuf::from(PROBE_MOUNT_ROOT).join(format!(
        "{}-{}",
        std::process::id(),
        device.file_name().and_then(|n| n.to_str()).unwrap_or("dev")
    ));
    std::fs::create_dir_all(&dir).ok()?;

    let mut opts = vec!["ro"];
    opts.extend_from_slice(extra_opts);
    let status = Command::new("mount")
        .args(["-t", fstype, "-o", &opts.join(",")])
        .arg(device)
        .arg(&dir)
        .output()
        .ok()?;
    if !status.status.success() {
        let _ = std::fs::remove_dir(&dir);
        return None;
    }

    let result = f(&dir);

    let _ = Command::new("umount").arg(&dir).output();
    let _ = Command::new("umount").args(["-l"]).arg(&dir).output();
    let _ = std::fs::remove_dir(&dir);
    Some(result)
}

/// What boots from this ESP: Windows Boot Manager and/or the titles of
/// BLS entries (which is where bootc-composefs systems put their name).
fn probe_esp(root: &Path) -> Option<DetectedOs> {
    let mut names: Vec<String> = Vec::new();
    let mut kind = OsKind::Linux;

    if root.join("EFI/Microsoft/Boot/bootmgfw.efi").exists() {
        names.push("Windows Boot Manager".into());
        kind = OsKind::Windows;
    }

    if let Ok(entries) = std::fs::read_dir(root.join("loader/entries")) {
        let mut titles: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "conf"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|content| {
                content
                    .lines()
                    .find_map(|l| l.strip_prefix("title ").map(|t| t.trim().to_owned()))
            })
            .collect();
        titles.sort();
        titles.dedup();
        names.extend(titles.into_iter().take(2));
    }

    if names.is_empty() {
        return None;
    }
    if names.len() > 1 {
        kind = OsKind::Unknown; // mixed loaders share this ESP
    }
    Some(DetectedOs {
        pretty_name: names.join(" + "),
        kind,
    })
}

/// PRETTY_NAME from os-release under a mounted root.
fn read_os_release(root: &Path) -> Option<DetectedOs> {
    for rel in ["etc/os-release", "usr/lib/os-release"] {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        for line in content.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                let name = v.trim().trim_matches('"').to_owned();
                if !name.is_empty() {
                    return Some(DetectedOs {
                        pretty_name: name,
                        kind: OsKind::Linux,
                    });
                }
            }
        }
    }
    None
}
