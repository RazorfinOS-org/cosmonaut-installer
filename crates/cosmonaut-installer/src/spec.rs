//! GUI-side install spec — collected piece-by-piece across wizard pages,
//! converted to the daemon's flat wire form right before `Install()` is
//! called.

use std::path::PathBuf;

use cosmonaut_engine::Encryption;

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct DraftSpec {
    pub image: Option<String>,
    pub disk: Option<PathBuf>,
    pub encryption: EncryptionChoice,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EncryptionChoice {
    #[default]
    None,
    LuksPassphrase,
    Tpm2Luks,
    Tpm2LuksPassphrase,
}

impl EncryptionChoice {
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "No encryption",
            Self::LuksPassphrase => "LUKS — passphrase",
            Self::Tpm2Luks => "LUKS — TPM2 only",
            Self::Tpm2LuksPassphrase => "LUKS — TPM2 + passphrase",
        }
    }

    pub fn needs_passphrase(&self) -> bool {
        matches!(self, Self::LuksPassphrase | Self::Tpm2LuksPassphrase)
    }
}

#[derive(Debug, Clone)]
pub struct FinalSpec {
    pub image: String,
    pub disk: PathBuf,
    pub hostname: String,
    pub encryption: Encryption,
}

impl FinalSpec {
    /// Convert into the daemon's flat `(disk, image, hostname, enc_type, enc_arg)` tuple.
    pub fn to_wire(&self) -> (String, String, String, String, String) {
        let (et, ea) = match &self.encryption {
            Encryption::None => ("none".to_owned(), String::new()),
            Encryption::LuksPassphrase { passphrase } => {
                ("luks-passphrase".to_owned(), passphrase.clone())
            }
            Encryption::Tpm2Luks => ("tpm2-luks".to_owned(), String::new()),
            Encryption::Tpm2LuksPassphrase { passphrase } => {
                ("tpm2-luks-passphrase".to_owned(), passphrase.clone())
            }
        };
        (
            self.disk.to_string_lossy().into_owned(),
            self.image.clone(),
            self.hostname.clone(),
            et,
            ea,
        )
    }
}
