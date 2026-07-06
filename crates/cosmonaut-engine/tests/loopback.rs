//! Loopback-device integration tests for the partition/format/mount
//! layers — the code where bugs eat disks.
//!
//! All `#[ignore]`d: they need root and real block-device tooling
//! (losetup, sfdisk, mkfs.*, cryptsetup). Run serially via:
//!
//!     just test-engine
//!
//! which compiles this target as the invoking user, then sudo-runs the
//! test executable with `--ignored --test-threads=1`.

mod common;

use std::path::Path;
use std::process::Command;

use cosmonaut_engine::probe;
use cosmonaut_engine::{PartitionAction, PartitionPlan};
use tokio::sync::mpsc;

use common::{device_sha256, require_root, LoopDisk};

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Drive `partition::run` through the public engine surface. The
/// partition module is private, so tests reach it via a tiny shim the
/// engine exposes for integration testing.
async fn run_plan(
    disk: &Path,
    plan: &PartitionPlan,
) -> anyhow::Result<cosmonaut_engine::testing::PartitionSetInfo> {
    let (tx, mut rx) = mpsc::channel(256);
    // Drain events so the sender never blocks.
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = cosmonaut_engine::testing::run_partition_plan(disk, plan, tx).await;
    let _ = drain.await;
    result
}

fn seed_gpt(disk: &LoopDisk, script: &str) {
    let mut child = Command::new("sfdisk")
        .arg(&disk.device)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn sfdisk");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success(), "seeding sfdisk failed");
    disk.settle();
}

#[tokio::test]
#[ignore]
async fn erase_disk_layout_matches_legacy() {
    if !require_root() {
        return;
    }
    let disk = LoopDisk::new(20 * GIB);
    let set = run_plan(&disk.device, &PartitionPlan::EraseDisk)
        .await
        .expect("erase plan");
    disk.settle();

    let info = probe::probe_disk_blocking(&disk.device).expect("probe");
    assert_eq!(info.partitions.len(), 3, "ESP + boot + root");
    assert_eq!(info.partitions[0].size_bytes, 512 * MIB);
    assert_eq!(info.partitions[1].size_bytes, GIB);
    assert_eq!(
        info.partitions[2].part_type.as_deref(),
        Some("4f68bce3-e8cd-4db1-96e7-fbcaf984b709"),
        "root partition carries the Linux-root GUID"
    );
    assert!(set.format_esp);
    assert!(set.boot.is_some(), "erase layout keeps the legacy /boot");
}

#[tokio::test]
#[ignore]
async fn free_space_reuses_esp_and_preserves_partitions() {
    if !require_root() {
        return;
    }
    let disk = LoopDisk::new(20 * GIB);
    // ESP + 2 GiB data, rest free.
    seed_gpt(
        &disk,
        "label: gpt\nsize=512MiB, type=uefi, name=ESP\nsize=2GiB, type=linux, name=data\n",
    );
    let info = probe::probe_disk_blocking(&disk.device).expect("probe");
    let gap = info.largest_gap().expect("has a gap");
    let data_dev = info.partitions[1].path.clone();
    let before = device_sha256(&data_dev);

    let set = run_plan(
        &disk.device,
        &PartitionPlan::FreeSpace {
            gap_start_bytes: gap.start_bytes,
            gap_size_bytes: gap.size_bytes,
        },
    )
    .await
    .expect("free-space plan");
    disk.settle();

    assert!(!set.format_esp, "existing ESP must be kept");
    assert!(set.boot.is_none(), "non-erase layouts have no /boot");
    let after_info = probe::probe_disk_blocking(&disk.device).expect("probe");
    assert_eq!(after_info.partitions.len(), 3, "ESP + data + new root");
    assert_eq!(
        device_sha256(&data_dev),
        before,
        "existing partition contents must be untouched"
    );
}

#[tokio::test]
#[ignore]
async fn free_space_rejects_stale_gap() {
    if !require_root() {
        return;
    }
    let disk = LoopDisk::new(20 * GIB);
    seed_gpt(&disk, "label: gpt\nsize=512MiB, type=uefi, name=ESP\n");
    // A gap that doesn't exist (overlaps the ESP).
    let err = run_plan(
        &disk.device,
        &PartitionPlan::FreeSpace {
            gap_start_bytes: MIB,
            gap_size_bytes: 19 * GIB,
        },
    )
    .await
    .expect_err("overlapping gap must be rejected");
    assert!(
        err.to_string().contains("not free space"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
#[ignore]
async fn custom_delete_reuse_create() {
    if !require_root() {
        return;
    }
    let disk = LoopDisk::new(24 * GIB);
    // ESP + junk (to delete) + keeper + free space.
    seed_gpt(
        &disk,
        "label: gpt\n\
         size=512MiB, type=uefi, name=ESP\n\
         size=1GiB, type=linux, name=junk\n\
         size=2GiB, type=linux, name=keeper\n",
    );
    let info = probe::probe_disk_blocking(&disk.device).expect("probe");
    let esp = info.partitions[0].path.clone();
    let junk = info.partitions[1].path.clone();
    // Identify junk by extent, not node path: sfdisk reuses freed
    // partition numbers, so /dev/loopXp2 may legitimately reappear as
    // the newly created root.
    let junk_extent = (
        info.partitions[1].start_bytes,
        info.partitions[1].size_bytes,
    );
    let keeper = info.partitions[2].path.clone();
    let keeper_before = device_sha256(&keeper);
    let gap = info.largest_gap().expect("gap");

    let set = run_plan(
        &disk.device,
        &PartitionPlan::Custom {
            actions: vec![
                PartitionAction::UseAsEsp {
                    device: esp.clone(),
                    format: false,
                },
                PartitionAction::Delete {
                    device: junk.clone(),
                },
                PartitionAction::CreateRoot {
                    start_bytes: gap.start_bytes,
                    size_bytes: None,
                },
            ],
        },
    )
    .await
    .expect("custom plan");
    disk.settle();

    assert_eq!(set.esp, esp);
    assert!(!set.format_esp);
    let after = probe::probe_disk_blocking(&disk.device).expect("probe");
    assert!(
        !after
            .partitions
            .iter()
            .any(|p| (p.start_bytes, p.size_bytes) == junk_extent),
        "junk partition extent must be gone"
    );
    assert_eq!(
        device_sha256(&keeper),
        keeper_before,
        "keeper partition must be untouched"
    );
    let root = after
        .partitions
        .iter()
        .find(|p| p.path == set.root)
        .expect("new root exists");
    assert_eq!(
        root.part_type.as_deref(),
        Some("4f68bce3-e8cd-4db1-96e7-fbcaf984b709")
    );
}

#[tokio::test]
#[ignore]
async fn custom_rejects_undersized_root() {
    if !require_root() {
        return;
    }
    let disk = LoopDisk::new(12 * GIB);
    seed_gpt(
        &disk,
        "label: gpt\nsize=512MiB, type=uefi, name=ESP\nsize=2GiB, type=linux, name=small\n",
    );
    let info = probe::probe_disk_blocking(&disk.device).expect("probe");
    let esp = info.partitions[0].path.clone();
    let small = info.partitions[1].path.clone();

    let err = run_plan(
        &disk.device,
        &PartitionPlan::Custom {
            actions: vec![
                PartitionAction::UseAsEsp {
                    device: esp,
                    format: false,
                },
                PartitionAction::UseAsRoot { device: small },
            ],
        },
    )
    .await
    .expect_err("2 GiB root must be rejected");
    assert!(
        err.to_string().contains("too small"),
        "unexpected error: {err}"
    );
}
