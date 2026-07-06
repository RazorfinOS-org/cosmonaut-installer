//! cosmonaut-installer-cli — headless driver for the cosmonaut DBus
//! daemon. Builds a `cosmonaut_engine::InstallSpec` from CLI args and
//! calls `InstallJson()` on the system bus, streaming `StepChanged`
//! and `LogLine` signals to stdout.
//!
//! Powers the QEMU-based PR gate — same install path as the GUI but
//! scriptable.

use anyhow::{bail, Context, Result};
use clap::Parser;
use cosmonaut_engine::{Encryption, InstallSpec, PartitionPlan};
use futures_util::StreamExt;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;
use zbus::proxy;

#[derive(Parser, Debug)]
#[command(version, about = "Drive cosmonaut-installer-daemon over DBus")]
struct Args {
    /// OCI image reference to install (e.g. ngcr.io/razorfinos/cosmic:nightly).
    #[arg(long)]
    image: String,

    /// Whole-disk device (e.g. /dev/vdb).
    #[arg(long)]
    disk: String,

    /// Hostname for the deployed system. Defaults to "cosmic"; first-boot
    /// setup wizard lets the user rename.
    #[arg(long, default_value = "cosmic")]
    hostname: String,

    /// LUKS passphrase. If supplied, encryption type is `luks-passphrase`.
    #[arg(long, conflicts_with_all = ["tpm2_luks", "tpm2_luks_passphrase"])]
    luks_passphrase: Option<String>,

    /// TPM2-only LUKS (no recovery passphrase). Mutually exclusive.
    #[arg(long, conflicts_with_all = ["luks_passphrase", "tpm2_luks_passphrase"])]
    tpm2_luks: bool,

    /// TPM2 LUKS with a recovery passphrase. Mutually exclusive.
    #[arg(long, conflicts_with_all = ["luks_passphrase", "tpm2_luks"])]
    tpm2_luks_passphrase: Option<String>,

    /// Partition plan as JSON (serde form of `PartitionPlan`), e.g.
    /// `{"mode":"free-space","gap_start_bytes":...,"gap_size_bytes":...}`.
    /// Defaults to erase-disk.
    #[arg(long)]
    plan_json: Option<String>,
}

#[proxy(
    interface = "dev.cosmonaut.Installer1",
    default_service = "dev.cosmonaut.Installer1",
    default_path = "/dev/cosmonaut/Installer1"
)]
trait Installer {
    async fn install_json(&self, spec_json: &str) -> zbus::Result<()>;

    async fn cancel(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn current_step(&self) -> zbus::Result<String>;

    #[zbus(signal)]
    fn step_changed(&self, step: &str, detail: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn log_line(&self, stream: &str, line: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn progress(&self, percent: u8, step: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn completed(&self, success: bool, error: &str) -> zbus::Result<()>;
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let encryption = derive_encryption(&args)?;
    let plan: PartitionPlan = match &args.plan_json {
        Some(json) => serde_json::from_str(json).context("parsing --plan-json")?,
        None => PartitionPlan::EraseDisk,
    };
    let spec = InstallSpec {
        disk: PathBuf::from(&args.disk),
        image: args.image.clone(),
        hostname: args.hostname.clone(),
        encryption,
        plan,
    };
    let spec_json = serde_json::to_string(&spec).context("serializing spec")?;

    let conn = zbus::Connection::system()
        .await
        .context("connecting to system DBus")?;
    let proxy = InstallerProxy::new(&conn)
        .await
        .context("creating installer proxy")?;

    // Subscribe to signals BEFORE calling Install so we don't miss the
    // first StepChanged.
    let mut step_stream = proxy.receive_step_changed().await?;
    let mut log_stream = proxy.receive_log_line().await?;
    let mut progress_stream = proxy.receive_progress().await?;
    let mut completed_stream = proxy.receive_completed().await?;

    let signals_done = tokio::sync::Notify::new();
    let signals_done = std::sync::Arc::new(signals_done);

    let step_printer = {
        let done = signals_done.clone();
        tokio::spawn(async move {
            while let Some(sig) = step_stream.next().await {
                if let Ok(args) = sig.args() {
                    eprintln!("== step: {} :: {}", args.step, args.detail);
                }
            }
            done.notify_waiters();
        })
    };
    let log_printer = tokio::spawn(async move {
        while let Some(sig) = log_stream.next().await {
            if let Ok(args) = sig.args() {
                println!("[{}] {}", args.stream, args.line);
            }
        }
    });
    let progress_printer = tokio::spawn(async move {
        while let Some(sig) = progress_stream.next().await {
            if let Ok(args) = sig.args() {
                eprintln!("== progress: {}% ({})", args.percent, args.step);
            }
        }
    });
    let completed_watcher = tokio::spawn(async move {
        if let Some(sig) = completed_stream.next().await {
            if let Ok(args) = sig.args() {
                if args.success {
                    eprintln!("== completed: ok");
                } else {
                    eprintln!("== completed: error -- {}", args.error);
                }
            }
        }
    });

    eprintln!("== calling InstallJson on dev.cosmonaut.Installer1 ==");
    let result = proxy.install_json(&spec_json).await;

    // Wait briefly for any in-flight signals to drain.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), completed_watcher).await;
    step_printer.abort();
    log_printer.abort();
    progress_printer.abort();

    match result {
        Ok(()) => {
            eprintln!("== install ok ==");
            Ok(())
        }
        Err(e) => {
            eprintln!("== install error: {e}");
            Err(e.into())
        }
    }
}

fn derive_encryption(args: &Args) -> Result<Encryption> {
    if let Some(p) = &args.luks_passphrase {
        if p.is_empty() {
            bail!("--luks-passphrase must be non-empty");
        }
        return Ok(Encryption::LuksPassphrase {
            passphrase: p.clone(),
        });
    }
    if args.tpm2_luks {
        return Ok(Encryption::Tpm2Luks);
    }
    if let Some(p) = &args.tpm2_luks_passphrase {
        if p.is_empty() {
            bail!("--tpm2-luks-passphrase must be non-empty");
        }
        return Ok(Encryption::Tpm2LuksPassphrase {
            passphrase: p.clone(),
        });
    }
    Ok(Encryption::None)
}
