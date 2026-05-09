//! Step 8: inject `rd.luks.uuid=<UUID>` into BLS boot entries on the
//! deployed system's /boot/loader/entries/*.conf, so initrd unlocks the
//! root volume at boot.
//!
//! BLS = Boot Loader Specification. systemd-boot reads each .conf file
//! and merges its `options` line into the kernel cmdline.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::mount::TARGET_ROOT;

pub fn inject_luks_uuid(luks_uuid: &str) -> Result<()> {
    let entries_dir = PathBuf::from(TARGET_ROOT).join("boot/loader/entries");
    let karg = format!("rd.luks.uuid={luks_uuid}");

    if !entries_dir.exists() {
        anyhow::bail!(
            "BLS entries directory does not exist: {} — bootc install may not have written entries yet",
            entries_dir.display()
        );
    }

    let mut touched = 0;
    for entry in fs::read_dir(&entries_dir).with_context(|| format!("read_dir {entries_dir:?}"))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("conf") {
            continue;
        }

        let content = fs::read_to_string(&path).with_context(|| format!("read {path:?}"))?;
        let mut updated = String::with_capacity(content.len() + karg.len() + 16);
        let mut saw_options = false;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("options ") {
                saw_options = true;
                if rest.split_whitespace().any(|t| t == karg) {
                    // Already injected; pass through verbatim.
                    updated.push_str(line);
                } else {
                    updated.push_str("options ");
                    updated.push_str(rest);
                    updated.push(' ');
                    updated.push_str(&karg);
                }
            } else {
                updated.push_str(line);
            }
            updated.push('\n');
        }
        if !saw_options {
            updated.push_str("options ");
            updated.push_str(&karg);
            updated.push('\n');
        }
        fs::write(&path, updated).with_context(|| format!("write {path:?}"))?;
        touched += 1;
    }

    if touched == 0 {
        anyhow::bail!("no BLS entries found under {}", entries_dir.display());
    }
    Ok(())
}
