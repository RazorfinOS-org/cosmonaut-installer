//! GUI-side install-spec helpers. The wire type is
//! `cosmonaut_engine::InstallSpec` itself (serialized to JSON for the
//! daemon's `InstallJson` method); this module only keeps the wizard's
//! intermediate choices.

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

/// Which partitioning mode the disk page has selected. Maps onto
/// `cosmonaut_engine::PartitionPlan` once the concrete gap/actions are
/// known.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PartitionModeChoice {
    /// Erase the whole disk (default, always available).
    #[default]
    EraseDisk,
    /// Install into the largest free-space gap.
    FreeSpace,
    /// Per-partition role assignment on a dedicated page.
    Custom,
}

impl PartitionModeChoice {
    pub fn label(&self) -> &'static str {
        match self {
            Self::EraseDisk => "Erase entire disk",
            Self::FreeSpace => "Install into free space",
            Self::Custom => "Custom layout",
        }
    }
}
