//! Install orchestrator. Wraps the privileged install pipeline:
//! partition → format → LUKS → mkfs → mount → bootc install → BLS inject → finalize.
//!
//! Phase 1 will flesh this out. Phase 0 only declares the shape so the
//! workspace resolves.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSpec {
    pub disk: PathBuf,
    pub image: String,
    pub encryption: Encryption,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Encryption {
    None,
    LuksPassphrase { passphrase: String },
    Tpm2Luks,
    Tpm2LuksPassphrase { passphrase: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Step {
    Partition,
    Format,
    Luks,
    Mkfs,
    Mount,
    Bootc,
    Bls,
    Finalize,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not implemented (Phase 0 stub)")]
    NotImplemented,
}

pub async fn install(_spec: InstallSpec) -> Result<(), EngineError> {
    Err(EngineError::NotImplemented)
}
