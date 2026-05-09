//! Client side of the `dev.cosmonaut.Installer1` interface. Used by the
//! progress page to drive an install and stream signals back as
//! [`DaemonEvent`]s.

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::proxy;

#[proxy(
    interface = "dev.cosmonaut.Installer1",
    default_service = "dev.cosmonaut.Installer1",
    default_path = "/dev/cosmonaut/Installer1"
)]
trait Installer {
    async fn install(
        &self,
        disk: &str,
        image: &str,
        hostname: &str,
        enc_type: &str,
        enc_arg: &str,
    ) -> zbus::Result<()>;

    async fn cancel(&self) -> zbus::Result<bool>;

    async fn is_online(&self) -> zbus::Result<bool>;

    async fn is_tpm2_available(&self) -> zbus::Result<bool>;

    async fn scan_wifi(&self) -> zbus::Result<Vec<(String, String, u32, bool)>>;

    async fn connect_wifi(&self, ssid: &str, passphrase: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn step_changed(&self, step: &str, detail: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn log_line(&self, stream: &str, line: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn completed(&self, success: bool, error: &str) -> zbus::Result<()>;
}

#[derive(Debug, Clone)]
pub struct WifiNetwork {
    pub ssid: String,
    pub security: String,
    pub signal: u8,
    pub connected: bool,
}

/// Async helper used by App's startup probe — queries the daemon for
/// `is_online()`. Returns `Ok(true)` if a default IPv4 route exists,
/// `Ok(false)` if not, `Err` on DBus failure.
pub async fn is_online() -> Result<bool, String> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| format!("system bus: {e}"))?;
    let proxy = InstallerProxy::new(&conn)
        .await
        .map_err(|e| format!("proxy: {e}"))?;
    proxy
        .is_online()
        .await
        .map_err(|e| format!("is_online: {e}"))
}

/// Probe whether the host has a TPM2 device. Errors (DBus unreachable,
/// daemon not present) are treated as "not available" by the caller.
pub async fn is_tpm2_available() -> Result<bool, String> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| format!("system bus: {e}"))?;
    let proxy = InstallerProxy::new(&conn)
        .await
        .map_err(|e| format!("proxy: {e}"))?;
    proxy
        .is_tpm2_available()
        .await
        .map_err(|e| format!("is_tpm2_available: {e}"))
}

/// Trigger an iwd scan via the daemon and return the network list.
pub async fn scan_wifi() -> Result<Vec<WifiNetwork>, String> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| format!("system bus: {e}"))?;
    let proxy = InstallerProxy::new(&conn)
        .await
        .map_err(|e| format!("proxy: {e}"))?;
    let raw = proxy
        .scan_wifi()
        .await
        .map_err(|e| format!("scan_wifi: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|(ssid, security, signal, connected)| WifiNetwork {
            ssid,
            security,
            signal: signal.min(u32::from(u8::MAX)) as u8,
            connected,
        })
        .collect())
}

/// Connect to `ssid` with `passphrase`. Blocks until iwd reports
/// success/failure.
pub async fn connect_wifi(ssid: String, passphrase: String) -> Result<(), String> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| format!("system bus: {e}"))?;
    let proxy = InstallerProxy::new(&conn)
        .await
        .map_err(|e| format!("proxy: {e}"))?;
    proxy
        .connect_wifi(&ssid, &passphrase)
        .await
        .map_err(|e| format!("connect_wifi: {e}"))
}

/// Events forwarded from DBus signals to the GUI's `update()` loop.
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    Step { step: String, detail: String },
    Log { stream: String, line: String },
    Completed { success: bool, error: String },
    /// Connection-level error (couldn't reach the daemon, name owner died, etc.)
    ConnectionError(String),
}

/// Spawn the install on a background tokio task. Streams every signal
/// into `tx` until the daemon emits `Completed`. Returns immediately.
///
/// `cancel_rx` lets the GUI request cancellation; we forward to the
/// daemon's `Cancel()` method when it fires.
pub fn spawn_install(
    disk: String,
    image: String,
    hostname: String,
    enc_type: String,
    enc_arg: String,
    tx: mpsc::UnboundedSender<DaemonEvent>,
    mut cancel_rx: tokio::sync::oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        let conn = match zbus::Connection::system().await {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(DaemonEvent::ConnectionError(format!(
                    "connecting to system bus: {e}"
                )));
                return;
            }
        };
        let proxy = match InstallerProxy::new(&conn).await {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.send(DaemonEvent::ConnectionError(format!(
                    "creating proxy: {e}"
                )));
                return;
            }
        };

        let mut step_stream = match proxy.receive_step_changed().await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(DaemonEvent::ConnectionError(format!(
                    "subscribing to StepChanged: {e}"
                )));
                return;
            }
        };
        let mut log_stream = match proxy.receive_log_line().await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(DaemonEvent::ConnectionError(format!(
                    "subscribing to LogLine: {e}"
                )));
                return;
            }
        };

        // Cancel forwarder.
        let proxy_for_cancel = proxy.clone();
        tokio::spawn(async move {
            if (&mut cancel_rx).await.is_ok() {
                let _ = proxy_for_cancel.cancel().await;
            }
        });

        // Drive the install on its own task; meanwhile stream signals.
        let proxy_for_install = proxy.clone();
        let install_handle = tokio::spawn(async move {
            proxy_for_install
                .install(&disk, &image, &hostname, &enc_type, &enc_arg)
                .await
        });

        let step_tx = tx.clone();
        let log_tx = tx.clone();
        let step_task = tokio::spawn(async move {
            while let Some(sig) = step_stream.next().await {
                if let Ok(args) = sig.args() {
                    let _ = step_tx.send(DaemonEvent::Step {
                        step: args.step.to_owned(),
                        detail: args.detail.to_owned(),
                    });
                }
            }
        });
        let log_task = tokio::spawn(async move {
            while let Some(sig) = log_stream.next().await {
                if let Ok(args) = sig.args() {
                    let _ = log_tx.send(DaemonEvent::Log {
                        stream: args.stream.to_owned(),
                        line: args.line.to_owned(),
                    });
                }
            }
        });

        match install_handle.await {
            Ok(Ok(())) => {
                let _ = tx.send(DaemonEvent::Completed {
                    success: true,
                    error: String::new(),
                });
            }
            Ok(Err(e)) => {
                let _ = tx.send(DaemonEvent::Completed {
                    success: false,
                    error: e.to_string(),
                });
            }
            Err(e) => {
                let _ = tx.send(DaemonEvent::Completed {
                    success: false,
                    error: format!("install task join: {e}"),
                });
            }
        }

        step_task.abort();
        log_task.abort();
    });
}

/// Best-effort reboot via logind. Spawned by the Done page's auto-reboot
/// timer.
pub fn spawn_reboot() {
    tokio::spawn(async move {
        let conn = match zbus::Connection::system().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(?e, "logind connection failed");
                return;
            }
        };
        let r = conn
            .call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "Reboot",
                &(false,),
            )
            .await;
        if let Err(e) = r {
            tracing::error!(?e, "logind Reboot failed");
        }
    });
}
