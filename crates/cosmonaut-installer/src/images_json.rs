//! Parser for the image catalog at `/etc/cosmonaut-installer/images.json`.
//!
//! Schema is the nested tree consumed by tuna-installer historically; we
//! preserved it so downstream junctions can keep their existing
//! `cosmic-images-json.bst` overrides working. Nodes are either branches
//! (have `children`) or leaves (have `imgref`).
//!
//! The picker presents all leaves flat. If there's exactly one, the
//! image page auto-skips (the user's choice is already made).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const PRIMARY_PATH: &str = "/etc/cosmonaut-installer/images.json";
/// Compat: the historical tuna-installer path. Read as fallback so we
/// can demo on existing ISOs that haven't been re-baked.
const FALLBACK_PATH: &str = "/etc/bootc-installer/images.json";
/// Dev-only override. If set, the catalog is read from this path
/// instead. Useful for running the GUI on a host without baking files
/// into /etc.
const ENV_OVERRIDE: &str = "COSMONAUT_IMAGES_JSON";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub default_image: Option<String>,
    pub images: Vec<Node>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub imgref: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<Node>>,
}

#[derive(Debug, Clone)]
pub struct ImageOption {
    pub name: String,
    pub desc: Option<String>,
    pub imgref: String,
}

impl Catalog {
    /// Walk the tree and return leaves in source order.
    pub fn leaves(&self) -> Vec<ImageOption> {
        let mut out = Vec::new();
        for n in &self.images {
            collect_leaves(n, &mut out);
        }
        out
    }
}

fn collect_leaves(node: &Node, out: &mut Vec<ImageOption>) {
    if let Some(imgref) = &node.imgref {
        out.push(ImageOption {
            name: node.name.clone(),
            desc: node.desc.clone(),
            imgref: imgref.clone(),
        });
    }
    if let Some(children) = &node.children {
        for c in children {
            collect_leaves(c, out);
        }
    }
}

/// Read the catalog from disk. Returns Ok(None) if no catalog file exists
/// at any known path. Resolution order: `$COSMONAUT_IMAGES_JSON` if set,
/// then `/etc/cosmonaut-installer/images.json`, then the historical
/// `/etc/bootc-installer/images.json`.
pub fn load() -> Result<Option<Catalog>, std::io::Error> {
    let mut paths: Vec<PathBuf> = Vec::with_capacity(3);
    if let Ok(p) = std::env::var(ENV_OVERRIDE) {
        if !p.is_empty() {
            paths.push(PathBuf::from(p));
        }
    }
    paths.push(PathBuf::from(PRIMARY_PATH));
    paths.push(PathBuf::from(FALLBACK_PATH));

    for path in paths {
        if path.exists() {
            let data = std::fs::read(&path)?;
            let catalog: Catalog = serde_json::from_slice(&data)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            tracing::info!(path = %path.display(), "loaded images catalog");
            return Ok(Some(catalog));
        }
    }
    tracing::warn!("no images.json at any known path");
    Ok(None)
}
