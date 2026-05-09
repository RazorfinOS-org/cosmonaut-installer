//! Step 6: skopeo-export the source image to an OCI layout, then run
//! `bootc install to-filesystem` against the mounted target.
//!
//! Matches `tuna-os/fisherman`'s `bootcDirect` path
//! (`fisherman/internal/install/bootc.go`).

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::runner;
use crate::Event;

pub use crate::mount::TARGET_ROOT;

/// Where skopeo writes the OCI layout for `--source-imgref oci:…`.
const OCI_CACHE_DIR: &str = "/run/cosmonaut/scratch/oci-cache";

pub async fn run(
    image: &str,
    events: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
) -> Result<()> {
    tokio::fs::create_dir_all("/run/cosmonaut/scratch")
        .await
        .context("mkdir scratch")?;

    // 1. skopeo copy <image> oci:/run/cosmonaut/scratch/oci-cache
    let dest = format!("oci:{OCI_CACHE_DIR}");
    let skopeo_args: [&str; 3] = ["copy", image, &dest];
    let copy_fut = runner::run("skopeo", &skopeo_args, events);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            anyhow::bail!("cancelled during skopeo copy");
        }
        r = copy_fut => r.context("skopeo copy")?,
    }

    // 2. bootc install to-filesystem ...
    let source_imgref = format!("oci:{OCI_CACHE_DIR}");
    let args = [
        "install",
        "to-filesystem",
        "--target-imgref",
        image,
        "--composefs-backend",
        "--source-imgref",
        &source_imgref,
        "--bootloader",
        "systemd",
        "--skip-finalize",
        TARGET_ROOT,
    ];

    let install_fut = runner::run("bootc", &args, events);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            anyhow::bail!("cancelled during bootc install");
        }
        r = install_fut => r.context("bootc install to-filesystem")?,
    }

    Ok(())
}
