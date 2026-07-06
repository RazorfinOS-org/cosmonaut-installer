//! Step 1: realize the [`PartitionPlan`] on the target disk.
//!
//! Three modes (see [`crate::PartitionPlan`]):
//!
//! - **EraseDisk** — wipe and write fisherman's 3-partition GPT layout:
//!   ESP 512 MiB FAT32, /boot 1 GiB ext4, root (rest) btrfs. The /boot
//!   partition is a legacy vestige (bootc's composefs backend keeps
//!   kernels + entries on the ESP — see docs/install-pipeline.md §S1);
//!   it stays in this mode until the 2-partition variant is E2E-proven.
//! - **FreeSpace** — append ESP-if-needed + root inside one gap,
//!   leaving existing partitions untouched (ESP + root only, the
//!   layout bootc's own `install to-disk` produces).
//! - **Custom** — explicit deletes/reuses/creates.
//!
//! Non-erase modes re-probe the disk and re-validate the plan against
//! reality before touching anything — the GUI's plan is a *request*,
//! not ground truth. New partition device paths are resolved by
//! re-reading the table and matching start sectors, because name-based
//! numbering conventions break on tables with existing partitions.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tokio::sync::mpsc;

use crate::probe::{self, DiskInfo};
use crate::runner;
use crate::{Event, PartitionAction, PartitionPlan, ESP_BYTES, ROOT_MIN_BYTES};

/// GPT type GUID for "Linux x86_64 root" — what bootc's auto-detection
/// expects when figuring out which partition to install to.
pub const GPT_LINUX_ROOT_X86_64: &str = "4f68bce3-e8cd-4db1-96e7-fbcaf984b709";
/// GPT type GUID for the EFI system partition.
pub const GPT_ESP: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

/// The block devices later steps operate on, plus what needs formatting.
#[derive(Debug, Clone)]
pub struct PartitionSet {
    pub esp: PathBuf,
    /// False when reusing an existing ESP without formatting (keeps
    /// other OSes' boot entries alive for multi-boot).
    pub format_esp: bool,
    /// Legacy ext4 /boot partition — only in the EraseDisk layout.
    pub boot: Option<PathBuf>,
    pub root: PathBuf,
}

pub async fn run(
    disk: &Path,
    plan: &PartitionPlan,
    events: &mpsc::Sender<Event>,
) -> Result<PartitionSet> {
    match plan {
        PartitionPlan::EraseDisk => erase_disk(disk, events).await,
        PartitionPlan::FreeSpace {
            gap_start_bytes,
            gap_size_bytes,
        } => free_space(disk, *gap_start_bytes, *gap_size_bytes, events).await,
        PartitionPlan::Custom { actions } => custom(disk, actions, events).await,
    }
}

/// The historical erase-everything path, kept verbatim.
async fn erase_disk(disk: &Path, events: &mpsc::Sender<Event>) -> Result<PartitionSet> {
    let disk_str = disk.to_str().context("disk path must be UTF-8")?.to_owned();

    // Wipe any existing filesystem signatures so sfdisk doesn't refuse.
    runner::run("wipefs", &["--all", &disk_str], events)
        .await
        .context("wipefs")?;

    // Partition table script piped to sfdisk via stdin.
    let script = "label: gpt\n\
                  size=512MiB, type=uefi, name=ESP\n\
                  size=1GiB,   type=linux, name=boot\n\
                  type=linux, name=root\n";
    runner::run_with_stdin(
        "sfdisk",
        &["--wipe", "always", &disk_str],
        script.as_bytes(),
        events,
    )
    .await
    .context("sfdisk")?;

    // Stamp the root partition with the canonical Linux root GUID.
    runner::run(
        "sfdisk",
        &["--part-type", &disk_str, "3", GPT_LINUX_ROOT_X86_64],
        events,
    )
    .await
    .context("sfdisk --part-type root")?;

    settle(events).await?;

    Ok(PartitionSet {
        esp: part_path(disk, 1),
        format_esp: true,
        boot: Some(part_path(disk, 2)),
        root: part_path(disk, 3),
    })
}

/// Create ESP-if-needed + root inside one free-space gap.
async fn free_space(
    disk: &Path,
    gap_start: u64,
    gap_size: u64,
    events: &mpsc::Sender<Event>,
) -> Result<PartitionSet> {
    let disk_str = disk.to_str().context("disk path must be UTF-8")?.to_owned();
    let info = probe_disk(disk).await?;
    let sector = info.sector_size;

    if info.table.as_deref() != Some("gpt") {
        bail!(
            "free-space install requires an existing GPT table on {} (found {:?}); \
             use erase mode for blank/MBR disks",
            disk.display(),
            info.table
        );
    }

    // The requested gap must be contained in a real, currently-free gap.
    let gap_ok = info.gaps.iter().any(|g| {
        gap_start >= g.start_bytes && gap_start + gap_size <= g.start_bytes + g.size_bytes
    });
    if !gap_ok {
        bail!(
            "requested gap ({gap_start}..+{gap_size}) is not free space on {} — \
             the disk changed since it was scanned; rescan and retry",
            disk.display()
        );
    }

    // Reuse an existing ESP when the disk has one (multi-boot friendly:
    // systemd-boot auto-lists other loaders on the shared ESP).
    let existing_esp = info
        .partitions
        .iter()
        .find(|p| p.part_type.as_deref() == Some(GPT_ESP))
        .map(|p| p.path.clone());

    let mut cursor = gap_start;
    let mut script = String::new();
    let esp_creating = existing_esp.is_none();
    if esp_creating {
        script.push_str(&format!(
            "start={}, size={}, type=uefi, name=ESP\n",
            cursor / sector,
            ESP_BYTES / sector
        ));
        cursor += ESP_BYTES;
    }
    let root_size = gap_start + gap_size - cursor;
    if root_size < ROOT_MIN_BYTES {
        bail!("free space too small for root: {root_size} < {ROOT_MIN_BYTES}");
    }
    let root_start = cursor;
    script.push_str(&format!(
        "start={}, size={}, type={GPT_LINUX_ROOT_X86_64}, name=root\n",
        root_start / sector,
        root_size / sector
    ));

    runner::run_with_stdin(
        "sfdisk",
        &["--append", &disk_str],
        script.as_bytes(),
        events,
    )
    .await
    .context("sfdisk --append")?;
    settle(events).await?;

    // Resolve the new device paths by start sector.
    let after = probe_disk(disk).await?;
    let root = partition_at(&after, root_start)
        .context("created root partition not found after append")?;
    let esp = match existing_esp {
        Some(p) => p,
        None => partition_at(&after, gap_start).context("created ESP not found after append")?,
    };

    Ok(PartitionSet {
        esp,
        format_esp: esp_creating,
        boot: None,
        root,
    })
}

/// Explicit per-partition roles: deletes, reuses, creates.
async fn custom(
    disk: &Path,
    actions: &[PartitionAction],
    events: &mpsc::Sender<Event>,
) -> Result<PartitionSet> {
    let disk_str = disk.to_str().context("disk path must be UTF-8")?.to_owned();
    let info = probe_disk(disk).await?;
    let sector = info.sector_size;

    if info.table.as_deref() != Some("gpt") {
        bail!(
            "custom layout requires an existing GPT table on {} (found {:?})",
            disk.display(),
            info.table
        );
    }

    // Referenced partitions must exist on this disk, now.
    let find = |device: &PathBuf| {
        info.partitions
            .iter()
            .find(|p| &p.path == device)
            .with_context(|| format!("{} does not exist on {}", device.display(), disk.display()))
    };

    // 1. Deletes, all in one sfdisk call.
    let mut delete_numbers = Vec::new();
    for action in actions {
        if let PartitionAction::Delete { device } = action {
            let p = find(device)?;
            if p.mounted {
                bail!("{} is mounted; refusing to delete", device.display());
            }
            delete_numbers.push(p.number.to_string());
        }
    }
    if !delete_numbers.is_empty() {
        let mut args = vec!["--delete", disk_str.as_str()];
        args.extend(delete_numbers.iter().map(String::as_str));
        runner::run("sfdisk", &args, events)
            .await
            .context("sfdisk --delete")?;
        settle(events).await?;
    }

    // 2. Creates, appended at explicit starts. Verify the ranges are
    // free *after* the deletes.
    let post_delete = probe_disk(disk).await?;
    let range_free = |start: u64, size: u64| {
        post_delete
            .gaps
            .iter()
            .any(|g| start >= g.start_bytes && start + size <= g.start_bytes + g.size_bytes)
    };

    let mut script = String::new();
    let mut created_root_start = None;
    let mut created_esp_start = None;
    for action in actions {
        match action {
            PartitionAction::CreateRoot {
                start_bytes,
                size_bytes,
            } => {
                let size = match size_bytes {
                    Some(s) => *s,
                    None => {
                        // Fill the gap the start falls in.
                        let gap = post_delete
                            .gaps
                            .iter()
                            .find(|g| {
                                *start_bytes >= g.start_bytes
                                    && *start_bytes < g.start_bytes + g.size_bytes
                            })
                            .context("create-root start is not inside a free gap")?;
                        gap.start_bytes + gap.size_bytes - *start_bytes
                    }
                };
                if size < ROOT_MIN_BYTES {
                    bail!("root partition too small: {size} < {ROOT_MIN_BYTES}");
                }
                if !range_free(*start_bytes, size) {
                    bail!("create-root range is not free space after deletes");
                }
                script.push_str(&format!(
                    "start={}, size={}, type={GPT_LINUX_ROOT_X86_64}, name=root\n",
                    start_bytes / sector,
                    size / sector
                ));
                created_root_start = Some(*start_bytes);
            }
            PartitionAction::CreateEsp { start_bytes } => {
                if !range_free(*start_bytes, ESP_BYTES) {
                    bail!("create-esp range is not free space after deletes");
                }
                script.push_str(&format!(
                    "start={}, size={}, type=uefi, name=ESP\n",
                    start_bytes / sector,
                    ESP_BYTES / sector
                ));
                created_esp_start = Some(*start_bytes);
            }
            _ => {}
        }
    }
    if !script.is_empty() {
        runner::run_with_stdin(
            "sfdisk",
            &["--append", &disk_str],
            script.as_bytes(),
            events,
        )
        .await
        .context("sfdisk --append")?;
        settle(events).await?;
    }

    // 3. Resolve devices + stamp reused partitions' type GUIDs.
    let after = probe_disk(disk).await?;
    let mut root = None;
    let mut esp = None;
    let mut format_esp = true;

    if let Some(start) = created_root_start {
        root = Some(partition_at(&after, start).context("created root not found after append")?);
    }
    if let Some(start) = created_esp_start {
        esp = Some(partition_at(&after, start).context("created ESP not found after append")?);
    }

    for action in actions {
        match action {
            PartitionAction::UseAsRoot { device } => {
                let p = after
                    .partitions
                    .iter()
                    .find(|p| &p.path == device)
                    .with_context(|| format!("{} vanished mid-plan", device.display()))?;
                if p.mounted {
                    bail!("{} is mounted; refusing to use as root", device.display());
                }
                if p.size_bytes < ROOT_MIN_BYTES {
                    bail!(
                        "{} too small for root: {} < {ROOT_MIN_BYTES}",
                        device.display(),
                        p.size_bytes
                    );
                }
                stamp_type(&disk_str, p.number, GPT_LINUX_ROOT_X86_64, events).await?;
                let dev = device.to_str().context("device utf-8")?;
                runner::run("wipefs", &["--all", dev], events)
                    .await
                    .context("wipefs root")?;
                root = Some(device.clone());
            }
            PartitionAction::UseAsEsp { device, format } => {
                let p = after
                    .partitions
                    .iter()
                    .find(|p| &p.path == device)
                    .with_context(|| format!("{} vanished mid-plan", device.display()))?;
                if p.mounted {
                    bail!("{} is mounted; refusing to use as ESP", device.display());
                }
                if p.part_type.as_deref() != Some(GPT_ESP) {
                    stamp_type(&disk_str, p.number, GPT_ESP, events).await?;
                }
                if *format {
                    let dev = device.to_str().context("device utf-8")?;
                    runner::run("wipefs", &["--all", dev], events)
                        .await
                        .context("wipefs esp")?;
                }
                format_esp = *format;
                esp = Some(device.clone());
            }
            _ => {}
        }
    }
    settle(events).await?;

    Ok(PartitionSet {
        esp: esp.context("plan produced no ESP")?,
        format_esp,
        boot: None,
        root: root.context("plan produced no root")?,
    })
}

async fn probe_disk(disk: &Path) -> Result<DiskInfo> {
    let disk = disk.to_owned();
    tokio::task::spawn_blocking(move || probe::probe_disk_blocking(&disk))
        .await
        .context("probe task join")?
}

/// Find the partition whose start matches `start_bytes`.
fn partition_at(info: &DiskInfo, start_bytes: u64) -> Option<PathBuf> {
    info.partitions
        .iter()
        .find(|p| p.start_bytes == start_bytes)
        .map(|p| p.path.clone())
}

async fn stamp_type(
    disk_str: &str,
    number: u32,
    guid: &str,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    let num = number.to_string();
    runner::run("sfdisk", &["--part-type", disk_str, &num, guid], events)
        .await
        .with_context(|| format!("sfdisk --part-type {num}"))
}

async fn settle(events: &mpsc::Sender<Event>) -> Result<()> {
    runner::run("udevadm", &["settle"], events)
        .await
        .context("udevadm settle")
}

/// Build `/dev/sda1` / `/dev/nvme0n1p1` / `/dev/vdb1` style paths.
/// Only trustworthy on a table we just created from scratch (erase
/// mode); everywhere else devices are resolved by start sector.
fn part_path(disk: &Path, num: u32) -> PathBuf {
    let s = disk.to_string_lossy();
    let suffix = if s.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        "p"
    } else {
        ""
    };
    PathBuf::from(format!("{s}{suffix}{num}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_paths_for_sata_and_nvme() {
        assert_eq!(
            part_path(Path::new("/dev/sda"), 1),
            PathBuf::from("/dev/sda1")
        );
        assert_eq!(
            part_path(Path::new("/dev/vdb"), 3),
            PathBuf::from("/dev/vdb3")
        );
        assert_eq!(
            part_path(Path::new("/dev/nvme0n1"), 2),
            PathBuf::from("/dev/nvme0n1p2")
        );
        assert_eq!(
            part_path(Path::new("/dev/mmcblk0"), 1),
            PathBuf::from("/dev/mmcblk0p1")
        );
    }
}
