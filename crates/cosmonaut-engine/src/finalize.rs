//! Step 9: fstrim the root, unmount everything, close LUKS if open.

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::mount::{self, TARGET_ROOT};
use crate::runner;
use crate::{luks, Event, LogStream};

pub async fn run(luks_in_use: bool, events: &mpsc::Sender<Event>) -> Result<()> {
    // fstrim the root mount. Genuinely best-effort: the backing device
    // may not support discard at all (spinning rust, some VM configs),
    // and dm-crypt mappings reject it unless opened with
    // --allow-discards. A failed trim must never fail the install —
    // observed live on a LUKS install, 2026-07-05.
    if let Err(e) = runner::run("fstrim", &["-v", TARGET_ROOT], events).await {
        let _ = events
            .send(Event::Log {
                stream: LogStream::Engine,
                line: format!("fstrim skipped: {e}"),
            })
            .await;
    }

    mount::unmount_all().await.context("unmount_all")?;

    if luks_in_use {
        luks::close().await.context("luks close")?;
    }

    Ok(())
}
