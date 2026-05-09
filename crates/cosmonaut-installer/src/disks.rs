//! Disk enumeration via `lsblk -ndo NAME,SIZE,MODEL,TYPE`.
//!
//! UDisks2 over zbus would be more idiomatic but pulls in a substantial
//! dep tree; lsblk is one subprocess and zero new crates. Phase 1+
//! upgrade is a non-event when we want hot-plug awareness.

use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Disk {
    pub path: PathBuf,
    pub size: String,
    pub model: String,
}

impl Disk {
    pub fn label(&self) -> String {
        let model = self.model.trim();
        if model.is_empty() {
            format!("{}  ({})", self.path.display(), self.size)
        } else {
            format!("{}  ({}, {})", self.path.display(), self.size, model)
        }
    }
}

/// Run lsblk and return the disks (TYPE=disk only). Block on the
/// subprocess — the call site is run on a background tokio task.
pub fn list_blocking() -> std::io::Result<Vec<Disk>> {
    let out = Command::new("lsblk")
        .args([
            "-ndo",
            "NAME,SIZE,MODEL,TYPE",
            "--paths",
        ])
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "lsblk exited with {}",
            out.status
        )));
    }

    let mut disks = Vec::new();
    for line in std::str::from_utf8(&out.stdout)
        .unwrap_or("")
        .lines()
    {
        let mut parts = line.split_whitespace();
        let name = parts.next();
        let size = parts.next();
        // model may have spaces; type is always last single token.
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            continue;
        }
        let last = *rest.last().unwrap();
        if last != "disk" {
            continue;
        }
        let model = if rest.len() > 1 {
            rest[..rest.len() - 1].join(" ")
        } else {
            String::new()
        };

        let (Some(name), Some(size)) = (name, size) else {
            continue;
        };
        disks.push(Disk {
            path: PathBuf::from(name),
            size: size.to_owned(),
            model,
        });
    }
    Ok(disks)
}
