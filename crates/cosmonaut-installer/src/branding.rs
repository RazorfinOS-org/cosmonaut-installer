//! Distro-overridable branding strings (window title, welcome copy, etc.).
//!
//! Distros consuming this installer should drop a `branding.json` at one
//! of the standard paths to set the visible product name without
//! forking. Resolution order (first wins):
//!
//! 1. `$COSMONAUT_BRANDING_JSON` (dev override).
//! 2. `/etc/cosmonaut-installer/branding.json` — system/admin override.
//!    Live-ISO builds (e.g. RazorfinOS junctioning from cosmic-build-meta)
//!    write here to override the upstream default.
//! 3. `/usr/share/cosmonaut-installer/branding.json` — vendor default
//!    shipped by cosmic-build-meta.
//! 4. Built-in fallback: `{ "name": "COSMIC" }`.
//!
//! Schema (all fields optional except `name`):
//!
//! ```json
//! {
//!   "name": "RazorfinOS"
//! }
//! ```
//!
//! Future-proofing: extra fields (tagline, logo path, accent color) can
//! be added later; unknown fields in the file are tolerated.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const PRIMARY_PATH: &str = "/etc/cosmonaut-installer/branding.json";
const VENDOR_PATH: &str = "/usr/share/cosmonaut-installer/branding.json";
const ENV_OVERRIDE: &str = "COSMONAUT_BRANDING_JSON";
const DEFAULT_NAME: &str = "COSMIC";

/// Raw on-disk schema for `branding.json`. Kept private; views work
/// against [`Branding`] which has the derived strings pre-computed so
/// the page views can hand out `&str` borrows instead of newly-allocated
/// `String`s every frame.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BrandingFile {
    /// Product name shown across the wizard ("COSMIC", "RazorfinOS", …).
    /// Optional in the file — falls back to the built-in default if
    /// missing or empty.
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Branding {
    pub name: String,
    pub installer_title: String,
    pub welcome_title: String,
    pub welcome_body: String,
    pub image_description: String,
    pub done_success_body: String,
}

impl Default for Branding {
    fn default() -> Self {
        Self::from_name(DEFAULT_NAME)
    }
}

impl Branding {
    fn from_name(name: &str) -> Self {
        Self {
            name: name.to_string(),
            installer_title: format!("{name} Installer"),
            welcome_title: format!("Welcome to {name}"),
            welcome_body: format!(
                "This installer will set up {name} on the disk you choose. \
                 Existing data on that disk will be erased."
            ),
            image_description: format!(
                "Pick which {name} image to install on the target disk."
            ),
            done_success_body: format!("{name} is installed. The system will reboot shortly."),
        }
    }

    /// Read the branding from disk, falling back to the built-in default
    /// if no file is found. Parse errors are logged and treated the same
    /// as a missing file so a malformed override never bricks the wizard.
    pub fn load() -> Self {
        let mut paths: Vec<PathBuf> = Vec::with_capacity(3);
        if let Ok(p) = std::env::var(ENV_OVERRIDE) {
            if !p.is_empty() {
                paths.push(PathBuf::from(p));
            }
        }
        paths.push(PathBuf::from(PRIMARY_PATH));
        paths.push(PathBuf::from(VENDOR_PATH));

        for path in paths {
            if !path.exists() {
                continue;
            }
            match std::fs::read(&path) {
                Ok(data) => match serde_json::from_slice::<BrandingFile>(&data) {
                    Ok(file) => {
                        let name = file
                            .name
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| DEFAULT_NAME.to_string());
                        tracing::info!(
                            path = %path.display(),
                            %name,
                            "loaded branding"
                        );
                        return Self::from_name(&name);
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "branding.json present but unparseable; continuing search"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "branding.json present but unreadable; continuing search"
                    );
                }
            }
        }
        tracing::info!(name = DEFAULT_NAME, "no branding.json found; using default");
        Self::default()
    }
}
