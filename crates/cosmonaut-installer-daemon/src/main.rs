//! cosmonaut-installer-daemon — system DBus service hosting the install
//! engine. Runs as root, registers `dev.cosmonaut.Installer1` on the
//! system bus, exposes a single typed `Install` method and a few signals
//! for progress.

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber::EnvFilter;

mod service;
mod wifi;

const BUS_NAME: &str = "dev.cosmonaut.Installer1";
const OBJECT_PATH: &str = "/dev/cosmonaut/Installer1";

/// Idle window after `Completed` before we exit (matches a one-shot DBus
/// service like PackageKit). The systemd unit is DBus-activated, so a
/// fresh install reactivates us cleanly.
const IDLE_EXIT_AFTER: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(bus = BUS_NAME, "starting cosmonaut-installer-daemon");

    let installer = service::Installer::new();
    let idle = installer.idle_signal();

    let conn = zbus::connection::Builder::system()
        .context("connecting to system bus")?
        .name(BUS_NAME)
        .context("requesting bus name")?
        .serve_at(OBJECT_PATH, installer)
        .context("registering object")?
        .build()
        .await
        .context("building connection")?;

    tracing::info!("DBus interface registered; awaiting calls");

    let mut sigterm = signal(SignalKind::terminate()).context("installing SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("installing SIGINT handler")?;

    tokio::select! {
        _ = idle_exit(idle) => {
            tracing::info!("idle window elapsed — exiting cleanly");
        }
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM — exiting");
        }
        _ = sigint.recv() => {
            tracing::info!("SIGINT — exiting");
        }
    }

    drop(conn);
    Ok(())
}

/// Wait for the engine to signal idleness, then debounce by [`IDLE_EXIT_AFTER`].
/// If a new install starts during the debounce window, the rx fires again
/// and we reset.
async fn idle_exit(mut idle_rx: tokio::sync::watch::Receiver<bool>) {
    // Initial state may be `idle = true` if no install is in flight at startup.
    loop {
        // Wait for an idle transition.
        if !*idle_rx.borrow_and_update() {
            // Currently busy — wait for it to flip to idle.
            if idle_rx.changed().await.is_err() {
                return; // sender dropped
            }
            continue;
        }
        // Idle — debounce.
        tokio::select! {
            _ = tokio::time::sleep(IDLE_EXIT_AFTER) => return,
            r = idle_rx.changed() => {
                if r.is_err() {
                    return;
                }
                // Transitioned (probably back to busy); loop and re-evaluate.
            }
        }
    }
}
