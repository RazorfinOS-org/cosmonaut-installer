//! Step 2: format the boot filesystems the plan calls for — the ESP
//! (FAT32, skipped when reusing one unformatted) and the legacy ext4
//! /boot partition (erase layout only).

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::partition::PartitionSet;
use crate::runner;
use crate::{Event, LogStream};

pub async fn run(parts: &PartitionSet, events: &mpsc::Sender<Event>) -> Result<()> {
    if parts.format_esp {
        let esp = parts.esp.to_str().context("esp path utf-8")?;
        runner::run("mkfs.fat", &["-F32", "-n", "ESP", esp], events)
            .await
            .context("mkfs.fat ESP")?;
    } else {
        let _ = events
            .send(Event::Log {
                stream: LogStream::Engine,
                line: format!("keeping existing ESP {} unformatted", parts.esp.display()),
            })
            .await;
    }

    if let Some(boot) = &parts.boot {
        let boot = boot.to_str().context("boot path utf-8")?;
        runner::run("mkfs.ext4", &["-F", "-L", "boot", boot], events)
            .await
            .context("mkfs.ext4 /boot")?;
    }

    Ok(())
}
