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
mod runner;

pub use bootc::TARGET_ROOT;

/// User-facing install specification. Daemon receives this from the GUI/CLI
/// and hands it to [`install`].
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
    StepChanged { step: Step, detail: String },
    Log { stream: LogStream, line: String },
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

    // 1. Partition
    emit_step(events, Step::Partition, format!("wiping and partitioning {}", spec.disk.display())).await;
    let parts = partition::run(&spec.disk, events)
        .await
        .map_err(EngineError::Partition)?;
    check_cancel!();

    // 2. Format ESP + /boot
    emit_step(events, Step::Format, "formatting ESP + /boot".into()).await;
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
    emit_step(events, Step::Mkfs, format!("mkfs.btrfs on {}", root_dev.display())).await;
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
    emit_step(events, Step::Hostname, format!("setting hostname to {}", spec.hostname)).await;
    hostname::run(&spec.hostname).map_err(EngineError::Hostname)?;
    check_cancel!();

    // 8. BLS inject (only when LUKS is configured)
    if spec.encryption.is_luks() {
        let luks_uuid = luks::partition_uuid(&parts.root)
            .await
            .map_err(EngineError::Luks)?;
        emit_step(events, Step::Bls, format!("injecting rd.luks.uuid={luks_uuid}")).await;
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

fn validate_spec(spec: &InstallSpec) -> Result<(), EngineError> {
    if !spec.disk.starts_with("/dev/") {
        return Err(EngineError::InvalidSpec(format!(
            "disk must be a /dev/ path, got {:?}",
            spec.disk
        )));
    }
    if spec.image.trim().is_empty() {
        return Err(EngineError::InvalidSpec("image must be non-empty".into()));
    }
    if spec.hostname.trim().is_empty() {
        return Err(EngineError::InvalidSpec("hostname must be non-empty".into()));
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
