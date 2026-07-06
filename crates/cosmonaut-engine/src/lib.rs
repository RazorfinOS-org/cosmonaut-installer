//! Install orchestrator. Runs the pipeline:
//! partition → format → LUKS → mkfs → mount → bootc → hostname → BLS inject → finalize.
//!
//! Modeled on `tuna-os/fisherman`'s install logic but specialised to our
//! opinionated profile (btrfs + composefs + systemd-boot, optional LUKS).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod bls;
mod bootc;
mod finalize;
mod format;
mod hostname;
mod luks;
mod mkfs;
mod mount;
mod partition;
pub mod probe;
mod runner;

pub use bootc::TARGET_ROOT;

/// Integration-test surface (loopback tests drive the partition layer
/// directly without running the whole pipeline). Not a stable API.
#[doc(hidden)]
pub mod testing {
    use std::path::{Path, PathBuf};

    use tokio::sync::mpsc;

    use crate::{partition, Event, PartitionPlan};

    #[derive(Debug, Clone)]
    pub struct PartitionSetInfo {
        pub esp: PathBuf,
        pub format_esp: bool,
        pub boot: Option<PathBuf>,
        pub root: PathBuf,
    }

    pub async fn run_partition_plan(
        disk: &Path,
        plan: &PartitionPlan,
        events: mpsc::Sender<Event>,
    ) -> anyhow::Result<PartitionSetInfo> {
        let set = partition::run(disk, plan, &events).await?;
        Ok(PartitionSetInfo {
            esp: set.esp,
            format_esp: set.format_esp,
            boot: set.boot,
            root: set.root,
        })
    }
}

/// User-facing install specification. Daemon receives this from the GUI/CLI
/// (as JSON over DBus) and hands it to [`install`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSpec {
    /// Whole-disk device, e.g. `/dev/vdb`.
    pub disk: PathBuf,
    /// OCI image reference to install, e.g. `ngcr.io/razorfinos/cosmic:nightly`.
    pub image: String,
    /// Hostname to write into `/etc/hostname` of the deployed system.
    /// Defaults to `"cosmic"`.
    pub hostname: String,
    /// Encryption strategy. Defaults to [`Encryption::None`].
    pub encryption: Encryption,
    /// How to place the install on `disk`. Defaults to
    /// [`PartitionPlan::EraseDisk`] (the historical behavior), so specs
    /// serialized by older clients keep working.
    #[serde(default)]
    pub plan: PartitionPlan,
}

/// Where the install's partitions come from. The filesystem profile is
/// fixed (btrfs+composefs root, FAT32 ESP, systemd-boot) — a plan only
/// controls *placement*.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum PartitionPlan {
    /// Wipe the disk, create the full layout. The default.
    #[default]
    EraseDisk,
    /// Create ESP-if-needed + root inside one free-space gap; existing
    /// partitions are not touched. Byte values must name a gap reported
    /// by [`probe`] (1 MiB aligned).
    FreeSpace {
        gap_start_bytes: u64,
        gap_size_bytes: u64,
    },
    /// Explicit per-partition roles. Partitions without an action are
    /// left alone.
    Custom { actions: Vec<PartitionAction> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PartitionAction {
    /// Delete this partition (its space becomes usable by `create-*`).
    Delete { device: PathBuf },
    /// Use this existing partition as the ESP. `format: false` keeps
    /// its contents (multi-boot: other OSes' loaders stay).
    UseAsEsp { device: PathBuf, format: bool },
    /// Use this existing partition as root — it will be wiped and
    /// formatted btrfs (LUKS-wrapped when encryption is on).
    UseAsRoot { device: PathBuf },
    /// Create the root partition in free space. `size_bytes: None`
    /// fills the gap the start falls in.
    CreateRoot {
        start_bytes: u64,
        size_bytes: Option<u64>,
    },
    /// Create a 512 MiB ESP in free space.
    CreateEsp { start_bytes: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Encryption {
    None,
    LuksPassphrase { passphrase: String },
    Tpm2Luks,
    Tpm2LuksPassphrase { passphrase: String },
}

impl Encryption {
    pub fn is_luks(&self) -> bool {
        !matches!(self, Encryption::None)
    }
}

/// One discrete pipeline step. Fires as a [`Event::StepChanged`] when the
/// engine begins each phase.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    Partition,
    Format,
    Luks,
    Mkfs,
    Mount,
    Bootc,
    Hostname,
    Bls,
    Finalize,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Partition => "partition",
            Self::Format => "format",
            Self::Luks => "luks",
            Self::Mkfs => "mkfs",
            Self::Mount => "mount",
            Self::Bootc => "bootc",
            Self::Hostname => "hostname",
            Self::Bls => "bls",
            Self::Finalize => "finalize",
        }
    }
}

/// Events streamed out of [`install`] to whoever's listening (the daemon
/// re-broadcasts these as DBus signals).
#[derive(Debug, Clone)]
pub enum Event {
    StepChanged {
        step: Step,
        detail: String,
    },
    Log {
        stream: LogStream,
        line: String,
    },
    /// Overall progress estimate, 0–100. Step-weighted baseline plus
    /// per-layer granularity inside the bootc step (the long one).
    /// Monotonic per install; absent updates mean "still working".
    Progress {
        percent: u8,
        step: Step,
    },
}

impl Step {
    /// Baseline percent when this step *starts*. The bootc step spans
    /// 10–90 and interpolates by copied image layers (see `bootc.rs`);
    /// everything else is effectively instant by comparison.
    pub fn start_percent(self) -> u8 {
        match self {
            Self::Partition => 2,
            Self::Format => 4,
            Self::Luks => 6,
            Self::Mkfs => 8,
            Self::Mount => 9,
            Self::Bootc => 10,
            Self::Hostname => 90,
            Self::Bls => 93,
            Self::Finalize => 95,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
    Engine,
}

impl LogStream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Engine => "engine",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    #[error("partition: {0}")]
    Partition(#[source] anyhow::Error),
    #[error("format: {0}")]
    Format(#[source] anyhow::Error),
    #[error("luks: {0}")]
    Luks(#[source] anyhow::Error),
    #[error("mkfs: {0}")]
    Mkfs(#[source] anyhow::Error),
    #[error("mount: {0}")]
    Mount(#[source] anyhow::Error),
    #[error("bootc: {0}")]
    Bootc(#[source] anyhow::Error),
    #[error("hostname: {0}")]
    Hostname(#[source] anyhow::Error),
    #[error("bls: {0}")]
    Bls(#[source] anyhow::Error),
    #[error("finalize: {0}")]
    Finalize(#[source] anyhow::Error),
    #[error("cancelled by user")]
    Cancelled,
}

/// Run the full install pipeline. Pushes [`Event`]s into `events` as it
/// progresses. Honors `cancel` between steps and (for the bootc step,
/// which is the long one) inside the subprocess wait.
///
/// Returns `Ok(())` on a clean install, `Err(EngineError::Cancelled)` if
/// the caller cancelled, or another `EngineError` variant on a real
/// failure. On any error path the engine attempts a best-effort
/// teardown (unmount, luksClose) before returning.
pub async fn install(
    spec: InstallSpec,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
) -> Result<(), EngineError> {
    validate_spec(&spec)?;

    // Best-effort teardown helper — runs whether we're cancelling, erroring,
    // or finished. Idempotent.
    let teardown_disk = spec.disk.clone();
    let teardown_encryption = spec.encryption.clone();
    let teardown = || async move {
        let _ = mount::unmount_all().await;
        if teardown_encryption.is_luks() {
            let _ = luks::close().await;
        }
        let _ = teardown_disk; // silence unused warning if future use disappears
    };

    let result = run_pipeline(&spec, &events, &cancel).await;

    if let Err(ref e) = result {
        emit_log(
            &events,
            LogStream::Engine,
            format!("pipeline failed: {e}; running cleanup"),
        )
        .await;
    }
    teardown().await;
    result
}

async fn run_pipeline(
    spec: &InstallSpec,
    events: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
) -> Result<(), EngineError> {
    macro_rules! check_cancel {
        () => {
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
        };
    }

    // 0. Preflight cleanup — a previous failed/cancelled install may have
    // left the target tree mounted or the LUKS mapper open (teardown is
    // best-effort). Clearing them here makes Retry robust. All no-ops on
    // a clean first run.
    emit_log(
        events,
        LogStream::Engine,
        "preflight: clearing stale mounts/mappers".into(),
    )
    .await;
    let _ = mount::unmount_all().await;
    let _ = luks::close().await;
    let _ = runner::run("udevadm", &["settle"], events).await;
    check_cancel!();

    // 1. Partition
    emit_step(
        events,
        Step::Partition,
        format!(
            "partitioning {} ({})",
            spec.disk.display(),
            spec.plan.describe()
        ),
    )
    .await;
    let parts = partition::run(&spec.disk, &spec.plan, events)
        .await
        .map_err(EngineError::Partition)?;
    check_cancel!();

    // 2. Format ESP (+ /boot when the layout has one)
    emit_step(events, Step::Format, "formatting boot filesystems".into()).await;
    format::run(&parts, events)
        .await
        .map_err(EngineError::Format)?;
    check_cancel!();

    // 3. LUKS (skipped for Encryption::None)
    let root_dev = if spec.encryption.is_luks() {
        emit_step(events, Step::Luks, "configuring LUKS".into()).await;
        let dev = luks::open(&parts.root, &spec.encryption, events)
            .await
            .map_err(EngineError::Luks)?;
        check_cancel!();
        dev
    } else {
        parts.root.clone()
    };

    // 4. mkfs.btrfs on root
    emit_step(
        events,
        Step::Mkfs,
        format!("mkfs.btrfs on {}", root_dev.display()),
    )
    .await;
    mkfs::run(&root_dev, events)
        .await
        .map_err(EngineError::Mkfs)?;
    check_cancel!();

    // 5. Mount
    emit_step(events, Step::Mount, "mounting target".into()).await;
    mount::run(&root_dev, &parts, events)
        .await
        .map_err(EngineError::Mount)?;
    check_cancel!();

    // 6. Bootc — the long one. Cancellable inside.
    emit_step(events, Step::Bootc, format!("installing {}", spec.image)).await;
    bootc::run(&spec.image, events, cancel)
        .await
        .map_err(EngineError::Bootc)?;
    check_cancel!();

    // 7. Hostname
    emit_step(
        events,
        Step::Hostname,
        format!("setting hostname to {}", spec.hostname),
    )
    .await;
    hostname::run(&spec.hostname).map_err(EngineError::Hostname)?;
    check_cancel!();

    // 8. BLS inject (only when LUKS is configured)
    if spec.encryption.is_luks() {
        let luks_uuid = luks::partition_uuid(&parts.root)
            .await
            .map_err(EngineError::Luks)?;
        emit_step(
            events,
            Step::Bls,
            format!("injecting rd.luks.name={luks_uuid}=root"),
        )
        .await;
        bls::inject_luks_uuid(&luks_uuid).map_err(EngineError::Bls)?;
        check_cancel!();
    }

    // 9. Finalize — fstrim + unmount + luksClose
    emit_step(events, Step::Finalize, "fstrim + unmount".into()).await;
    finalize::run(spec.encryption.is_luks(), events)
        .await
        .map_err(EngineError::Finalize)?;

    Ok(())
}

/// Smallest root filesystem the engine will create. The GUI applies a
/// stricter, image-size-aware minimum; this floor is the engine-side
/// backstop against nonsense plans.
pub const ROOT_MIN_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Size of a newly created ESP.
pub const ESP_BYTES: u64 = 512 * 1024 * 1024;

impl PartitionPlan {
    pub fn describe(&self) -> &'static str {
        match self {
            Self::EraseDisk => "erase whole disk",
            Self::FreeSpace { .. } => "use free space",
            Self::Custom { .. } => "custom layout",
        }
    }
}

fn validate_spec(spec: &InstallSpec) -> Result<(), EngineError> {
    let err = |m: String| Err(EngineError::InvalidSpec(m));
    if !spec.disk.starts_with("/dev/") {
        return err(format!("disk must be a /dev/ path, got {:?}", spec.disk));
    }
    if spec.image.trim().is_empty() {
        return err("image must be non-empty".into());
    }
    if spec.hostname.trim().is_empty() {
        return err("hostname must be non-empty".into());
    }

    // Static plan-shape checks. Device-level checks (partitions exist,
    // gaps really free) happen in partition::run against a fresh probe —
    // never trust a stale or hostile client.
    match &spec.plan {
        PartitionPlan::EraseDisk => {}
        PartitionPlan::FreeSpace { gap_size_bytes, .. } => {
            if *gap_size_bytes < ROOT_MIN_BYTES {
                return err(format!(
                    "free-space gap too small: {gap_size_bytes} < {ROOT_MIN_BYTES} minimum"
                ));
            }
        }
        PartitionPlan::Custom { actions } => {
            let mut roots = 0;
            let mut esps = 0;
            let mut devices: Vec<&PathBuf> = Vec::new();
            for action in actions {
                match action {
                    PartitionAction::UseAsRoot { device } => {
                        roots += 1;
                        devices.push(device);
                    }
                    PartitionAction::CreateRoot { size_bytes, .. } => {
                        roots += 1;
                        if let Some(s) = size_bytes {
                            if *s < ROOT_MIN_BYTES {
                                return err(format!(
                                    "root partition too small: {s} < {ROOT_MIN_BYTES} minimum"
                                ));
                            }
                        }
                    }
                    PartitionAction::UseAsEsp { device, .. } => {
                        esps += 1;
                        devices.push(device);
                    }
                    PartitionAction::CreateEsp { .. } => esps += 1,
                    PartitionAction::Delete { device } => devices.push(device),
                }
            }
            if roots != 1 {
                return err(format!("plan must have exactly one root, got {roots}"));
            }
            if esps != 1 {
                return err(format!("plan must have exactly one ESP, got {esps}"));
            }
            let disk_prefix = spec.disk.to_string_lossy();
            let mut seen = std::collections::HashSet::new();
            for d in devices {
                if !seen.insert(d) {
                    return err(format!("device referenced twice in plan: {}", d.display()));
                }
                // String prefix, not Path::starts_with — the latter is
                // component-wise and "/dev/vdb3" is not a child of
                // "/dev/vdb" in path terms.
                if !d.to_string_lossy().starts_with(disk_prefix.as_ref()) {
                    return err(format!(
                        "plan references {} which is not on {}",
                        d.display(),
                        spec.disk.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

async fn emit_step(events: &mpsc::Sender<Event>, step: Step, detail: String) {
    let _ = events
        .send(Event::StepChanged {
            step,
            detail: detail.clone(),
        })
        .await;
    let _ = events
        .send(Event::Progress {
            percent: step.start_percent(),
            step,
        })
        .await;
    tracing::info!(step = step.as_str(), %detail, "step start");
}

async fn emit_log(events: &mpsc::Sender<Event>, stream: LogStream, line: String) {
    let _ = events
        .send(Event::Log {
            stream,
            line: line.clone(),
        })
        .await;
    tracing::debug!(target: "cosmonaut_engine::log", stream = stream.as_str(), %line);
}
