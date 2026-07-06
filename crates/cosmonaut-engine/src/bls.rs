//! Step 8: inject `rd.luks.name=<UUID>=root` into BLS boot entries of
//! the deployed system, so initrd unlocks the root volume at boot.
//!
//! BLS = Boot Loader Specification. systemd-boot reads each .conf file
//! and merges its `options` line into the kernel cmdline.
//!
//! Two findings from a real install (2026-07-05, bootc 1.15 — see
//! docs/install-pipeline.md):
//!
//! - bootc's composefs backend writes entries to the ESP at
//!   `loader/entries` (mounted at target/boot/efi), alongside the
//!   kernels under `EFI/Linux/bootc_composefs-<digest>/`. Older bootc
//!   wrote type-1 entries to `/boot/loader/entries`; we fall back to
//!   that path if the ESP has no entries dir.
//!
//! - The karg must be `rd.luks.name=<UUID>=root`, not `rd.luks.uuid=`.
//!   The cmdline bootc generates has no `root=` karg (composefs
//!   discovery), so systemd-gpt-auto-generator is active and creates
//!   its own `systemd-cryptsetup@root.service` for the LUKS partition
//!   (it carries the root-x86-64 GPT type GUID). With `rd.luks.uuid=`
//!   a *second* unit (`…@luks-<uuid>.service`) races it for the same
//!   device; the loser drops the boot to emergency mode. Naming our
//!   mapping `root` makes both generators produce the same unit, so
//!   there is exactly one unlock prompt and one mapping.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::mount::TARGET_ROOT;

pub fn inject_luks_uuid(luks_uuid: &str) -> Result<()> {
    let esp_entries = PathBuf::from(TARGET_ROOT).join("boot/efi/loader/entries");
    let boot_entries = PathBuf::from(TARGET_ROOT).join("boot/loader/entries");
    let entries_dir = if esp_entries.is_dir() {
        esp_entries
    } else if boot_entries.is_dir() {
        boot_entries
    } else {
        anyhow::bail!(
            "no BLS entries directory at {} or {} — bootc install may not have written entries yet",
            esp_entries.display(),
            boot_entries.display()
        );
    };
    let karg = format!("rd.luks.name={luks_uuid}=root");

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
