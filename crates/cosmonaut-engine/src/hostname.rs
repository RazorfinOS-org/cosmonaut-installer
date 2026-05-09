//! Step 7: write `/etc/hostname` into the deployed system.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::mount::TARGET_ROOT;

pub fn run(hostname: &str) -> Result<()> {
    let path = PathBuf::from(TARGET_ROOT).join("etc/hostname");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
    }
    std::fs::write(&path, format!("{}\n", hostname.trim()))
        .with_context(|| format!("write {path:?}"))?;
    Ok(())
}
