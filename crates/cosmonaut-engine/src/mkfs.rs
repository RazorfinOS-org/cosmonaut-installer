//! Step 4: mkfs.btrfs on the root partition (or LUKS mapper).

use std::path::Path;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::runner;
use crate::Event;

pub async fn run(root: &Path, events: &mpsc::Sender<Event>) -> Result<()> {
    let dev = root.to_str().context("root path utf-8")?;
    runner::run("mkfs.btrfs", &["-f", "-L", "cosmic-root", dev], events)
        .await
        .context("mkfs.btrfs")
}
