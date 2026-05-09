//! Step 2: format the ESP (FAT32) and the /boot partition (ext4).

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::partition::Partitions;
use crate::runner;
use crate::Event;

pub async fn run(parts: &Partitions, events: &mpsc::Sender<Event>) -> Result<()> {
    let esp = parts.esp.to_str().context("esp path utf-8")?;
    let boot = parts.boot.to_str().context("boot path utf-8")?;

    runner::run("mkfs.fat", &["-F32", "-n", "ESP", esp], events)
        .await
        .context("mkfs.fat ESP")?;

    runner::run("mkfs.ext4", &["-F", "-L", "boot", boot], events)
        .await
        .context("mkfs.ext4 /boot")?;

    Ok(())
}
