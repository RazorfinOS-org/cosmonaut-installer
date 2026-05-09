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

/// Transport prefixes skopeo recognises. If a user-supplied image ref
/// starts with one of these, pass it through verbatim; otherwise wrap
/// in `docker://` (the natural default for registry refs).
const SKOPEO_TRANSPORTS: &[&str] = &[
    "docker://",
    "docker-archive:",
    "docker-daemon:",
    "oci:",
    "oci-archive:",
    "containers-storage:",
    "dir:",
    "tarball:",
    "ostree:",
];

fn skopeo_source(image: &str) -> String {
    if SKOPEO_TRANSPORTS.iter().any(|p| image.starts_with(p)) {
        image.to_string()
    } else {
        format!("docker://{image}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_ref_gets_docker_prefix() {
        assert_eq!(
            skopeo_source("ngcr.io/foo/bar:nightly"),
            "docker://ngcr.io/foo/bar:nightly"
        );
        assert_eq!(
            skopeo_source("ghcr.io/example/image:v1.2.3"),
            "docker://ghcr.io/example/image:v1.2.3"
        );
    }

    #[test]
    fn explicit_transport_passes_through() {
        for s in [
            "docker://ngcr.io/foo:1",
            "oci:/tmp/foo",
            "containers-storage:localhost/foo:latest",
            "oci-archive:/tmp/foo.tar",
        ] {
            assert_eq!(skopeo_source(s), s);
        }
    }
}

pub async fn run(
    image: &str,
    events: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
) -> Result<()> {
    tokio::fs::create_dir_all("/run/cosmonaut/scratch")
        .await
        .context("mkdir scratch")?;

    // 1. skopeo copy <image> oci:/run/cosmonaut/scratch/oci-cache.
    // skopeo requires an explicit transport prefix on both ends; bare
    // registry refs like "ngcr.io/foo:nightly" error immediately with
    // "Invalid image name". Default to docker:// when no transport
    // prefix is present (matches the registry pull that bootc would
    // do on its own).
    let source = skopeo_source(image);
    let dest = format!("oci:{OCI_CACHE_DIR}");
    let skopeo_args: [&str; 3] = ["copy", &source, &dest];
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
