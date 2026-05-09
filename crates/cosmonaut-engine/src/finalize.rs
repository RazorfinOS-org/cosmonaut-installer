//! Step 9: fstrim the root, unmount everything, close LUKS if open.

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::mount::{self, TARGET_ROOT};
use crate::runner;
use crate::{luks, Event};

pub async fn run(luks_in_use: bool, events: &mpsc::Sender<Event>) -> Result<()> {
    // fstrim the root mount (best-effort; may not apply on all backings).
    runner::run("fstrim", &["-v", TARGET_ROOT], events)
        .await
        .context("fstrim")?;

    mount::unmount_all().await.context("unmount_all")?;

    if luks_in_use {
        luks::close().await.context("luks close")?;
    }

    Ok(())
}
