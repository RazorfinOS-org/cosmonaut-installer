//! Step 5 (and finalize teardown): mount and unmount the install target.
//!
//! Layout:
//!
//!   /run/cosmonaut/target              <- root (btrfs or LUKS-mapper)
//!   /run/cosmonaut/target/boot         <- /boot (ext4)
//!   /run/cosmonaut/target/boot/efi     <- ESP (FAT32)
//!
//! `bootc install to-filesystem` operates on the root mount; it discovers
//! /boot and /boot/efi by examining mountinfo for the path it's given.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::partition::Partitions;
use crate::runner;
use crate::Event;

/// Where we mount the target root. Beneath /run so it's a tmpfs path that
/// vanishes on reboot.
pub const TARGET_ROOT: &str = "/run/cosmonaut/target";

pub async fn run(
    root: &Path,
    parts: &Partitions,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    let target = PathBuf::from(TARGET_ROOT);
    let boot = target.join("boot");
    let efi = boot.join("efi");

    tokio::fs::create_dir_all(&target).await.context("mkdir target")?;

    mount_subprocess(root, &target, "btrfs", &[], events).await?;
    tokio::fs::create_dir_all(&boot).await.context("mkdir target/boot")?;

    mount_subprocess(&parts.boot, &boot, "ext4", &[], events).await?;
    tokio::fs::create_dir_all(&efi).await.context("mkdir target/boot/efi")?;

    mount_subprocess(&parts.esp, &efi, "vfat", &["umask=0077"], events).await?;
    Ok(())
}

/// Best-effort recursive unmount of the target tree. Used by both finalize
/// (success path) and teardown (cancel/error path). Idempotent — silently
/// no-ops if nothing's mounted.
pub async fn unmount_all() -> Result<()> {
    use tokio::process::Command;
    // -R = recursive (unmount all submounts under TARGET_ROOT first), -l =
    // lazy fall back if something's still busy. We don't propagate errors:
    // if nothing's mounted, umount returns non-zero and that's fine.
    let _ = Command::new("umount")
        .args(["-R", TARGET_ROOT])
        .output()
        .await;
    let _ = Command::new("umount")
        .args(["-Rl", TARGET_ROOT])
        .output()
        .await;
    Ok(())
}

async fn mount_subprocess(
    src: &Path,
    dst: &Path,
    fstype: &str,
    options: &[&str],
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    let src_s = src.to_str().context("src utf-8")?;
    let dst_s = dst.to_str().context("dst utf-8")?;
    let mut args: Vec<&str> = vec!["-t", fstype];
    let opt_str;
    if !options.is_empty() {
        opt_str = options.join(",");
        args.push("-o");
        args.push(&opt_str);
    }
    args.push(src_s);
    args.push(dst_s);
    runner::run("mount", &args, events)
        .await
        .with_context(|| format!("mount {src_s} → {dst_s}"))
}
