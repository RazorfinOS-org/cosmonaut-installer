//! Loopback-device fixture for engine integration tests.
//!
//! Requires root (losetup, mount, device nodes) — tests using it are
//! `#[ignore]`d and run via `just test-engine` (sudo). A sparse backing
//! file keeps disk usage tiny regardless of nominal size.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A loop device over a sparse temp file; detached + deleted on drop.
pub struct LoopDisk {
    pub device: PathBuf,
    backing: PathBuf,
}

impl LoopDisk {
    /// Create a sparse file of `size_bytes` and attach it with
    /// partition scanning (`-P`) so partition nodes appear.
    pub fn new(size_bytes: u64) -> LoopDisk {
        let backing = std::env::temp_dir().join(format!(
            "cosmonaut-loop-{}-{}.img",
            std::process::id(),
            unique()
        ));
        let f = std::fs::File::create(&backing).expect("create backing file");
        f.set_len(size_bytes).expect("set sparse size");
        drop(f);

        let out = Command::new("losetup")
            .args(["-fP", "--show"])
            .arg(&backing)
            .output()
            .expect("run losetup");
        assert!(
            out.status.success(),
            "losetup failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let device = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_owned());
        LoopDisk { device, backing }
    }

    /// Re-read the partition table + settle udev, so partition nodes
    /// (`/dev/loopXp1`) exist after sfdisk writes.
    pub fn settle(&self) {
        let _ = Command::new("partprobe").arg(&self.device).output();
        let _ = Command::new("udevadm").arg("settle").output();
    }

    /// `sfdisk --dump` text, for table comparisons.
    pub fn dump(&self) -> String {
        let out = Command::new("sfdisk")
            .arg("--dump")
            .arg(&self.device)
            .output()
            .expect("sfdisk --dump");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for LoopDisk {
    fn drop(&mut self) {
        let _ = Command::new("losetup").arg("-d").arg(&self.device).output();
        let _ = std::fs::remove_file(&self.backing);
    }
}

/// Monotonic-ish uniquifier without pulling in rand.
fn unique() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Skip helper: tests call this first and return early when not root.
pub fn require_root() -> bool {
    let uid = unsafe { libc_geteuid() };
    if uid != 0 {
        eprintln!("skipping: requires root (run via `just test-engine`)");
        return false;
    }
    true
}

extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

/// Sha256 of a partition's full contents via `sha256sum` (tool exists
/// wherever the loopback suite runs).
pub fn device_sha256(dev: &Path) -> String {
    let out = Command::new("sha256sum")
        .arg(dev)
        .output()
        .expect("sha256sum");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}
