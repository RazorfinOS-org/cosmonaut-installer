//! Disk/partition probing shared by the GUI (unprivileged fallback) and
//! the daemon (root path — Phase 3 adds ro-mount OS detection on top).
//!
//! Data comes from two subprocesses per disk:
//! - `lsblk -J -b -o …` — sizes, filesystems, labels, mount state
//! - `sfdisk --json <disk>` — sector-accurate partition starts/sizes,
//!   which lsblk cannot provide; needed for free-space (gap) math.
//!
//! Parsing is kept in pure functions over the JSON text so it can be
//! unit-tested against captured fixtures without block devices.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 1 MiB — all created partitions are aligned to this, so gaps are
/// reported pre-shrunk to aligned bounds.
pub const ALIGNMENT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiskInfo {
    pub path: PathBuf,
    pub model: String,
    pub size_bytes: u64,
    pub removable: bool,
    pub rotational: bool,
    /// GPT/MBR label reported by sfdisk, e.g. "gpt". None = blank disk
    /// (no recognized partition table).
    pub table: Option<String>,
    /// Logical sector size (512 unless sfdisk says otherwise).
    #[serde(default = "default_sector_size")]
    pub sector_size: u64,
    pub partitions: Vec<PartitionInfo>,
    /// Aligned free-space gaps usable for new partitions, largest not
    /// guaranteed first — ordered by on-disk position.
    pub gaps: Vec<Gap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartitionInfo {
    pub path: PathBuf,
    pub number: u32,
    pub start_bytes: u64,
    pub size_bytes: u64,
    pub fstype: Option<String>,
    pub label: Option<String>,
    /// GPT partition type GUID (lowercase) or MBR type byte as hex.
    pub part_type: Option<String>,
    pub part_uuid: Option<String>,
    /// Filesystem UUID.
    pub fs_uuid: Option<String>,
    pub mounted: bool,
    /// Filled by the daemon's OS probe (Phase 3); None from the plain
    /// lsblk/sfdisk path.
    pub detected_os: Option<DetectedOs>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetectedOs {
    pub pretty_name: String,
    pub kind: OsKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OsKind {
    Linux,
    Windows,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gap {
    pub start_bytes: u64,
    pub size_bytes: u64,
}

impl DiskInfo {
    /// Largest usable gap, if any.
    pub fn largest_gap(&self) -> Option<Gap> {
        self.gaps.iter().copied().max_by_key(|g| g.size_bytes)
    }
}

/// Human-readable size, binary units ("931.5 GiB").
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if (value - value.round()).abs() < 0.05 {
        format!("{} {}", value.round(), UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ---------- lsblk parsing ----------

#[derive(Debug, Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<LsblkDevice>,
}

#[derive(Debug, Deserialize)]
struct LsblkDevice {
    path: PathBuf,
    size: u64,
    #[serde(rename = "type")]
    dev_type: String,
    fstype: Option<String>,
    label: Option<String>,
    parttype: Option<String>,
    partuuid: Option<String>,
    uuid: Option<String>,
    model: Option<String>,
    #[serde(default)]
    rm: bool,
    #[serde(default)]
    rota: bool,
    #[serde(default)]
    mountpoints: Vec<Option<String>>,
}

const LSBLK_COLUMNS: &str =
    "PATH,SIZE,TYPE,FSTYPE,LABEL,PARTTYPE,PARTUUID,UUID,MODEL,RM,ROTA,MOUNTPOINTS";

// ---------- sfdisk parsing ----------

#[derive(Debug, Deserialize)]
struct SfdiskOutput {
    partitiontable: SfdiskTable,
}

#[derive(Debug, Deserialize)]
struct SfdiskTable {
    label: String,
    #[serde(default)]
    firstlba: Option<u64>,
    #[serde(default)]
    lastlba: Option<u64>,
    #[serde(default = "default_sector_size")]
    sectorsize: u64,
    #[serde(default)]
    partitions: Vec<SfdiskPartition>,
}

fn default_sector_size() -> u64 {
    512
}

#[derive(Debug, Deserialize)]
struct SfdiskPartition {
    node: PathBuf,
    start: u64,
    size: u64,
}

/// Partition number from its device node relative to the parent disk:
/// `/dev/sda3` -> 3, `/dev/nvme0n1p2` -> 2. Falls back to trailing
/// digits when the node doesn't start with the disk path.
pub fn partition_number(disk: &Path, node: &Path) -> Option<u32> {
    let node_str = node.to_str()?;
    let suffix = match disk.to_str().and_then(|d| node_str.strip_prefix(d)) {
        Some(s) => s,
        None => node_str.trim_start_matches(|c: char| !c.is_ascii_digit()),
    };
    suffix.trim_start_matches('p').parse().ok()
}

fn align_up(v: u64, align: u64) -> u64 {
    v.div_ceil(align) * align
}

fn align_down(v: u64, align: u64) -> u64 {
    (v / align) * align
}

/// Compute aligned free-space gaps from sfdisk's sector data. `first`/
/// `last` are the usable LBA range; partitions may be listed in any
/// order.
fn compute_gaps(
    first_lba: u64,
    last_lba: u64,
    sector_size: u64,
    parts: &[SfdiskPartition],
) -> Vec<Gap> {
    let mut spans: Vec<(u64, u64)> = parts
        .iter()
        .map(|p| (p.start, p.start + p.size)) // [start, end) in sectors
        .collect();
    spans.sort_unstable();

    let mut gaps = Vec::new();
    let mut cursor = first_lba;
    for (start, end) in spans {
        if start > cursor {
            push_gap(&mut gaps, cursor, start, sector_size);
        }
        cursor = cursor.max(end);
    }
    // lastlba is inclusive; the usable region ends after it.
    if last_lba + 1 > cursor {
        push_gap(&mut gaps, cursor, last_lba + 1, sector_size);
    }
    gaps
}

fn push_gap(gaps: &mut Vec<Gap>, start_sector: u64, end_sector: u64, sector_size: u64) {
    let start = align_up(start_sector * sector_size, ALIGNMENT_BYTES);
    let end = align_down(end_sector * sector_size, ALIGNMENT_BYTES);
    if end > start && end - start >= ALIGNMENT_BYTES {
        gaps.push(Gap {
            start_bytes: start,
            size_bytes: end - start,
        });
    }
}

/// Merge one disk's lsblk + sfdisk JSON into a [`DiskInfo`].
///
/// `sfdisk_json` is `None` for blank disks (sfdisk exits non-zero when
/// there's no partition table) — the whole disk is then one gap.
pub fn parse_disk(lsblk_json: &str, sfdisk_json: Option<&str>) -> Result<DiskInfo> {
    let lsblk: LsblkOutput = serde_json::from_str(lsblk_json).context("parsing lsblk JSON")?;
    let mut devices = lsblk.blockdevices.into_iter();
    // "loop" counts as a disk here so the loopback test suite (and any
    // losetup-backed target) probes like real hardware. Enumeration
    // (`list_disk_paths_blocking`) still offers only TYPE=disk devices.
    let disk = devices
        .next()
        .filter(|d| d.dev_type == "disk" || d.dev_type == "loop")
        .context("lsblk output does not start with a disk device")?;

    let sfdisk: Option<SfdiskOutput> = sfdisk_json
        .map(|s| serde_json::from_str(s).context("parsing sfdisk JSON"))
        .transpose()?;

    let (table, gaps, sector_data) = match &sfdisk {
        Some(out) => {
            let t = &out.partitiontable;
            let sector_size = t.sectorsize;
            // GPT always reports first/lastlba; MBR may not. Fall back
            // to a conventional 1 MiB start and the disk end.
            let first = t.firstlba.unwrap_or(ALIGNMENT_BYTES / sector_size);
            let last = t.lastlba.unwrap_or_else(|| disk.size / sector_size - 1);
            (
                Some(t.label.clone()),
                compute_gaps(first, last, sector_size, &t.partitions),
                Some((sector_size, &t.partitions)),
            )
        }
        None => (None, Vec::new(), None),
    };

    let mut partitions = Vec::new();
    for dev in devices.filter(|d| d.dev_type == "part") {
        let (start_bytes, size_bytes) = match &sector_data {
            Some((sector_size, parts)) => {
                match parts.iter().find(|p| p.node == dev.path) {
                    Some(p) => (p.start * sector_size, p.size * sector_size),
                    None => continue, // e.g. deleted-but-still-in-udev
                }
            }
            // No sfdisk data (unprivileged caller, or the table vanished
            // mid-probe): keep the partition with its lsblk size but an
            // unknown (zero) start. Gap math is skipped in this case.
            None => (0, dev.size),
        };
        partitions.push(PartitionInfo {
            number: partition_number(&disk.path, &dev.path).unwrap_or(0),
            path: dev.path,
            start_bytes,
            size_bytes,
            fstype: dev.fstype,
            label: dev.label,
            part_type: dev.parttype.map(|s| s.to_lowercase()),
            part_uuid: dev.partuuid,
            fs_uuid: dev.uuid,
            mounted: dev.mountpoints.iter().any(|m| m.is_some()),
            detected_os: None,
        });
    }
    partitions.sort_by_key(|p| p.start_bytes);

    // Truly blank disk (no table, no partitions): the whole usable area
    // is one gap. With partitions but no sector data we can't do gap
    // math, so `gaps` stays empty (computed above).
    let gaps = if sfdisk.is_none() && partitions.is_empty() {
        let mut g = Vec::new();
        push_gap(&mut g, ALIGNMENT_BYTES / 512, disk.size / 512, 512);
        g
    } else {
        gaps
    };

    Ok(DiskInfo {
        path: disk.path,
        model: disk.model.unwrap_or_default().trim().to_owned(),
        size_bytes: disk.size,
        removable: disk.rm,
        rotational: disk.rota,
        table,
        sector_size: sfdisk
            .as_ref()
            .map(|s| s.partitiontable.sectorsize)
            .unwrap_or(512),
        partitions,
        gaps,
    })
}

// ---------- subprocess drivers (blocking; call from spawn_blocking) ----------

/// List whole-disk device paths (`lsblk -J -b -o PATH,TYPE`).
pub fn list_disk_paths_blocking() -> Result<Vec<PathBuf>> {
    let out = Command::new("lsblk")
        .args(["-J", "-b", "-o", "PATH,TYPE"])
        .output()
        .context("running lsblk")?;
    anyhow::ensure!(out.status.success(), "lsblk exited with {}", out.status);

    #[derive(Deserialize)]
    struct Slim {
        blockdevices: Vec<SlimDev>,
    }
    #[derive(Deserialize)]
    struct SlimDev {
        path: PathBuf,
        #[serde(rename = "type")]
        dev_type: String,
    }

    let parsed: Slim = serde_json::from_slice(&out.stdout).context("parsing lsblk JSON")?;
    Ok(parsed
        .blockdevices
        .into_iter()
        .filter(|d| d.dev_type == "disk")
        .map(|d| d.path)
        .collect())
}

/// Probe one disk: lsblk for the device tree + sfdisk for sector layout.
pub fn probe_disk_blocking(disk: &Path) -> Result<DiskInfo> {
    let lsblk = Command::new("lsblk")
        .args(["-J", "-b", "-o", LSBLK_COLUMNS])
        .arg(disk)
        .output()
        .with_context(|| format!("running lsblk on {}", disk.display()))?;
    anyhow::ensure!(
        lsblk.status.success(),
        "lsblk {} exited with {}",
        disk.display(),
        lsblk.status
    );
    let lsblk_json = String::from_utf8_lossy(&lsblk.stdout).into_owned();

    let sfdisk = Command::new("sfdisk")
        .arg("--json")
        .arg(disk)
        .output()
        .with_context(|| format!("running sfdisk on {}", disk.display()))?;
    // Non-zero = no partition table (blank disk) — legitimate.
    let sfdisk_json = sfdisk
        .status
        .success()
        .then(|| String::from_utf8_lossy(&sfdisk.stdout).into_owned());

    parse_disk(&lsblk_json, sfdisk_json.as_deref())
}

/// Probe every whole disk on the system.
pub fn probe_disks_blocking() -> Result<Vec<DiskInfo>> {
    let mut disks = Vec::new();
    for path in list_disk_paths_blocking()? {
        match probe_disk_blocking(&path) {
            Ok(d) => disks.push(d),
            // A disk that vanished mid-probe (USB unplug) shouldn't kill
            // the whole listing.
            Err(e) => tracing::warn!(disk = %path.display(), error = %e, "probing disk failed"),
        }
    }
    Ok(disks)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from a Fedora host (values null without root: acceptable —
    // the daemon path runs as root and gets them populated).
    const LSBLK_FIXTURE: &str = r#"{
        "blockdevices": [
            {"path":"/dev/vdb","size":64424509440,"type":"disk","fstype":null,"label":null,"parttype":null,"partuuid":null,"uuid":null,"model":"virtio-disk","rm":false,"rota":false,"mountpoints":[]},
            {"path":"/dev/vdb1","size":536870912,"type":"part","fstype":"vfat","label":null,"parttype":"c12a7328-f81f-11d2-ba4b-00a0c93ec93b","partuuid":"aaaa-01","uuid":"1234-ABCD","rm":false,"rota":false,"mountpoints":[null]},
            {"path":"/dev/vdb2","size":1073741824,"type":"part","fstype":"ext4","label":"boot","parttype":"0fc63daf-8483-4772-8e79-3d69d8477de4","partuuid":"aaaa-02","uuid":"0fc06bbf-e032-48c5-97a9-3472abb7299f","rm":false,"rota":false,"mountpoints":["/mnt/boot"]},
            {"path":"/dev/vdb3","size":32212254720,"type":"part","fstype":"btrfs","label":"cosmic-root","parttype":"4f68bce3-e8cd-4db1-96e7-fbcaf984b709","partuuid":"aaaa-03","uuid":"10a0c499-fa2d-4d24-a993-16ac9cd106d8","rm":false,"rota":false,"mountpoints":[]}
        ]
    }"#;

    // Matching sfdisk layout: ESP 512M + boot 1G + root 30G on a 60G
    // disk, leaving a ~28.5G tail gap.
    const SFDISK_FIXTURE: &str = r#"{
        "partitiontable": {
            "label": "gpt",
            "id": "C9EF0CC2-C524-41EB-ABCE-52FA6A07AB51",
            "device": "/dev/vdb",
            "unit": "sectors",
            "firstlba": 2048,
            "lastlba": 125829086,
            "sectorsize": 512,
            "partitions": [
                {"node":"/dev/vdb1","start":2048,"size":1048576,"type":"C12A7328-F81F-11D2-BA4B-00A0C93EC93B","uuid":"AAAA-01","name":"ESP"},
                {"node":"/dev/vdb2","start":1050624,"size":2097152,"type":"0FC63DAF-8483-4772-8E79-3D69D8477DE4","uuid":"AAAA-02","name":"boot"},
                {"node":"/dev/vdb3","start":3147776,"size":62914560,"type":"4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709","uuid":"AAAA-03","name":"root"}
            ]
        }
    }"#;

    #[test]
    fn parses_disk_with_partitions_and_tail_gap() {
        let disk = parse_disk(LSBLK_FIXTURE, Some(SFDISK_FIXTURE)).unwrap();
        assert_eq!(disk.path, PathBuf::from("/dev/vdb"));
        assert_eq!(disk.model, "virtio-disk");
        assert_eq!(disk.table.as_deref(), Some("gpt"));
        assert_eq!(disk.partitions.len(), 3);

        let esp = &disk.partitions[0];
        assert_eq!(esp.number, 1);
        assert_eq!(esp.start_bytes, 2048 * 512);
        assert_eq!(esp.size_bytes, 512 * 1024 * 1024);
        assert_eq!(
            esp.part_type.as_deref(),
            Some("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
        );
        assert!(!esp.mounted);
        assert!(disk.partitions[1].mounted);

        // One tail gap: from end of vdb3 to lastlba, aligned.
        assert_eq!(disk.gaps.len(), 1);
        let gap = disk.gaps[0];
        assert_eq!(gap.start_bytes, (3147776 + 62914560) * 512);
        // 125829087 sectors * 512 aligned down to MiB.
        let end = align_down(125829087 * 512, ALIGNMENT_BYTES);
        assert_eq!(gap.size_bytes, end - gap.start_bytes);
    }

    #[test]
    fn gap_between_partitions_is_found_and_aligned() {
        // ESP at 2048, then a hole, then a partition at 4196352 (2 GiB in).
        let sfdisk = r#"{
            "partitiontable": {
                "label": "gpt", "id": "X", "device": "/dev/vdb",
                "unit": "sectors",
                "firstlba": 2048, "lastlba": 8390655, "sectorsize": 512,
                "partitions": [
                    {"node":"/dev/vdb1","start":2048,"size":1048576,"type":"T","uuid":"U","name":"ESP"},
                    {"node":"/dev/vdb2","start":4196352,"size":4194302,"type":"T","uuid":"U","name":"data"}
                ]
            }
        }"#;
        let lsblk = r#"{"blockdevices":[
            {"path":"/dev/vdb","size":4294967296,"type":"disk","fstype":null,"label":null,"parttype":null,"partuuid":null,"uuid":null,"model":null,"rm":false,"rota":false,"mountpoints":[]},
            {"path":"/dev/vdb1","size":536870912,"type":"part","fstype":null,"label":null,"parttype":null,"partuuid":null,"uuid":null,"rm":false,"rota":false,"mountpoints":[]},
            {"path":"/dev/vdb2","size":2147482624,"type":"part","fstype":null,"label":null,"parttype":null,"partuuid":null,"uuid":null,"rm":false,"rota":false,"mountpoints":[]}
        ]}"#;
        let disk = parse_disk(lsblk, Some(sfdisk)).unwrap();
        assert_eq!(disk.gaps.len(), 1);
        let gap = disk.gaps[0];
        // Hole spans sectors [1050624, 4196352) = bytes [537918464, 2148532224).
        assert_eq!(gap.start_bytes, align_up(1050624 * 512, ALIGNMENT_BYTES));
        assert_eq!(
            gap.start_bytes + gap.size_bytes,
            align_down(4196352 * 512, ALIGNMENT_BYTES)
        );
    }

    #[test]
    fn blank_disk_is_one_big_gap() {
        let lsblk = r#"{"blockdevices":[
            {"path":"/dev/vdc","size":10737418240,"type":"disk","fstype":null,"label":null,"parttype":null,"partuuid":null,"uuid":null,"model":"blank","rm":false,"rota":false,"mountpoints":[]}
        ]}"#;
        let disk = parse_disk(lsblk, None).unwrap();
        assert!(disk.table.is_none());
        assert!(disk.partitions.is_empty());
        assert_eq!(disk.gaps.len(), 1);
        assert_eq!(disk.gaps[0].start_bytes, ALIGNMENT_BYTES);
    }

    #[test]
    fn partition_numbers() {
        let n = |d: &str, p: &str| partition_number(Path::new(d), Path::new(p));
        assert_eq!(n("/dev/sda", "/dev/sda3"), Some(3));
        assert_eq!(n("/dev/nvme0n1", "/dev/nvme0n1p2"), Some(2));
        assert_eq!(n("/dev/mmcblk0", "/dev/mmcblk0p1"), Some(1));
        assert_eq!(n("/dev/vdb", "/dev/vdb12"), Some(12));
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(536870912), "512 MiB");
        assert_eq!(human_size(64424509440), "60 GiB");
        assert_eq!(human_size(1000204886016), "931.5 GiB");
    }
}
