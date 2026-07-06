//! Disk enumeration for the wizard, built on `cosmonaut_engine::probe`
//! (lsblk JSON + sfdisk sector layout).
//!
//! Running unprivileged (host-side dev runs), sfdisk can't read raw
//! devices, so partition starts/gaps may be missing — the probe
//! degrades gracefully. In the live env the GUI runs as the live user
//! which can read block devices; Phase 3 moves probing into the root
//! daemon (adding OS detection) with this as the fallback.

use cosmonaut_engine::probe::{self, human_size};

pub use cosmonaut_engine::probe::DiskInfo as Disk;

/// Display helpers for probe types, kept GUI-side so the engine stays
/// presentation-free.
pub trait DiskExt {
    fn label(&self) -> String;
}

impl DiskExt for Disk {
    fn label(&self) -> String {
        let size = human_size(self.size_bytes);
        if self.model.is_empty() {
            format!("{}  ({size})", self.path.display())
        } else {
            format!("{}  ({size}, {})", self.path.display(), self.model)
        }
    }
}

/// Probe all whole disks. Blocking — call from `spawn_blocking`.
pub fn list_blocking() -> anyhow::Result<Vec<Disk>> {
    probe::probe_disks_blocking()
}

/// Daemon probe (root: gap math + OS detection) with a local
/// unprivileged fallback for host-side dev runs without the daemon.
pub async fn probe_with_fallback() -> Result<Vec<Disk>, String> {
    match crate::daemon::probe_disks().await {
        Ok(disks) => Ok(disks),
        Err(e) => {
            tracing::warn!(error = %e, "daemon probe unavailable; falling back to local lsblk");
            tokio::task::spawn_blocking(list_blocking)
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
        }
    }
}
