//! Step 1: wipe and partition the target disk.
//!
//! Creates a 3-partition GPT layout matching fisherman's:
//!
//!   1. EFI System  — 512 MiB FAT32, mounted at `/boot/efi`
//!   2. /boot       — 1 GiB ext4, mounted at `/boot`
//!   3. Linux root  — remaining space, btrfs (optionally LUKS-wrapped)
//!
//! For composefs + systemd-boot the /boot partition is largely unused
//! (kernel + initrd live on the ESP), but we keep it for compatibility
//! with bootc's expected layout.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::runner;
use crate::Event;

/// GPT type GUID for "Linux x86_64 root" — what bootc's auto-detection
/// expects when figuring out which partition to install to.
pub const GPT_LINUX_ROOT_X86_64: &str = "4f68bce3-e8cd-4db1-96e7-fbcaf984b709";

#[derive(Debug, Clone)]
pub struct Partitions {
    pub esp: PathBuf,
    pub boot: PathBuf,
    pub root: PathBuf,
}

pub async fn run(disk: &Path, events: &mpsc::Sender<Event>) -> Result<Partitions> {
    let disk_str = disk
        .to_str()
        .context("disk path must be UTF-8")?
        .to_owned();

    // Wipe any existing filesystem signatures so sfdisk doesn't refuse.
    runner::run("wipefs", &["--all", &disk_str], events)
        .await
        .context("wipefs")?;

    // Partition table script piped to sfdisk via stdin.
    let script = "label: gpt\n\
                  size=512MiB, type=uefi, name=ESP\n\
                  size=1GiB,   type=linux, name=boot\n\
                  type=linux, name=root\n";
    runner::run_with_stdin("sfdisk", &["--wipe", "always", &disk_str], script.as_bytes(), events)
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

    // Settle udev so the partition device nodes appear.
    runner::run("udevadm", &["settle"], events)
        .await
        .context("udevadm settle")?;

    Ok(Partitions {
        esp: part_path(disk, 1),
        boot: part_path(disk, 2),
        root: part_path(disk, 3),
    })
}

/// Build `/dev/sda1` / `/dev/nvme0n1p1` / `/dev/vdb1` style paths.
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
        assert_eq!(part_path(Path::new("/dev/sda"), 1), PathBuf::from("/dev/sda1"));
        assert_eq!(part_path(Path::new("/dev/vdb"), 3), PathBuf::from("/dev/vdb3"));
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
