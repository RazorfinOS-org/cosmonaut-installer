//! Step 3 + 9 + BLS-side: LUKS configuration.
//!
//! Phase 1 implementation is a thin wrapper around the `cryptsetup`
//! subprocess. A future phase can swap in `libcryptsetup-rs` for typed
//! errors and finer-grained progress reporting.
//!
//! For TPM2 binding we shell out to `systemd-cryptenroll` after format.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::runner;
use crate::{Encryption, Event};

/// Name passed to `cryptsetup luksOpen`. The mapper device is at
/// `/dev/mapper/<MAPPER_NAME>` once opened.
pub const MAPPER_NAME: &str = "cosmonaut-root";

/// Format and open the root partition as a LUKS volume.
/// Returns the unlocked mapper device path.
pub async fn open(
    root_part: &Path,
    encryption: &Encryption,
    events: &mpsc::Sender<Event>,
) -> Result<PathBuf> {
    let root = root_part.to_str().context("root path utf-8")?;
    let mapper_path = PathBuf::from(format!("/dev/mapper/{MAPPER_NAME}"));

    match encryption {
        Encryption::None => {
            anyhow::bail!("luks::open called with Encryption::None");
        }
        Encryption::LuksPassphrase { passphrase } => {
            luks_format(root, passphrase, events).await?;
            luks_open(root, passphrase, events).await?;
        }
        Encryption::Tpm2Luks => {
            // Fisherman generates a one-time random passphrase, formats LUKS
            // with it, opens, then enrolls a TPM2 key and removes the
            // passphrase keyslot. We do the same.
            let throwaway = uuid::Uuid::new_v4().to_string();
            luks_format(root, &throwaway, events).await?;
            luks_open(root, &throwaway, events).await?;
            tpm2_enroll(root, Some(&throwaway), None, events).await?;
            cryptsetup_remove_keyslot(root, &throwaway, events).await?;
        }
        Encryption::Tpm2LuksPassphrase { passphrase } => {
            luks_format(root, passphrase, events).await?;
            luks_open(root, passphrase, events).await?;
            tpm2_enroll(root, Some(passphrase), None, events).await?;
        }
    }
    Ok(mapper_path)
}

/// Best-effort `cryptsetup luksClose`. Used by teardown / finalize.
pub async fn close() -> Result<()> {
    use tokio::process::Command;
    let _ = Command::new("cryptsetup")
        .args(["luksClose", MAPPER_NAME])
        .output()
        .await;
    Ok(())
}

/// Read the LUKS header UUID for the root partition. Used by the BLS
/// step to inject `rd.luks.uuid=<UUID>` into the kernel cmdline.
pub async fn partition_uuid(root_part: &Path) -> Result<String> {
    let root = root_part.to_str().context("root path utf-8")?;
    let uuid = runner::capture_stdout("cryptsetup", &["luksUUID", root]).await?;
    Ok(uuid)
}

async fn luks_format(
    root: &str,
    passphrase: &str,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    runner::run_with_stdin(
        "cryptsetup",
        &[
            "luksFormat",
            "--type",
            "luks2",
            "--batch-mode",
            "--key-file",
            "-",
            root,
        ],
        passphrase.as_bytes(),
        events,
    )
    .await
    .context("cryptsetup luksFormat")
}

async fn luks_open(
    root: &str,
    passphrase: &str,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    runner::run_with_stdin(
        "cryptsetup",
        &["luksOpen", "--key-file", "-", root, MAPPER_NAME],
        passphrase.as_bytes(),
        events,
    )
    .await
    .context("cryptsetup luksOpen")
}

async fn tpm2_enroll(
    root: &str,
    existing_pass: Option<&str>,
    pcrs: Option<&str>,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    let args: Vec<String> = vec![
        "--tpm2-device=auto".into(),
        format!("--tpm2-pcrs={}", pcrs.unwrap_or("7")),
        root.into(),
    ];
    let argstrs: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Some(p) = existing_pass {
        runner::run_with_stdin("systemd-cryptenroll", &argstrs, p.as_bytes(), events)
            .await
            .context("systemd-cryptenroll --tpm2-device=auto (with existing passphrase)")
    } else {
        runner::run("systemd-cryptenroll", &argstrs, events)
            .await
            .context("systemd-cryptenroll --tpm2-device=auto")
    }
}

async fn cryptsetup_remove_keyslot(
    root: &str,
    passphrase: &str,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    runner::run_with_stdin(
        "cryptsetup",
        &["luksRemoveKey", "--key-file", "-", root],
        passphrase.as_bytes(),
        events,
    )
    .await
    .context("cryptsetup luksRemoveKey")
}
