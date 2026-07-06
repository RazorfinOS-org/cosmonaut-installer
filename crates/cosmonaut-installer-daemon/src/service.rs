//! `dev.cosmonaut.Installer1` interface. Exposes one typed `Install`
//! method (blocking — returns when the install finishes), a `Cancel`
//! method, two read-only properties, and three signals.

use std::sync::Arc;

use cosmonaut_engine::{install, Event, InstallSpec, Step};
use tokio::sync::{mpsc, watch, Mutex};
use tokio_util::sync::CancellationToken;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

use crate::wifi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunState {
    Idle,
    Running,
    Done,
    Error,
}

impl RunState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Done => "done",
            Self::Error => "error",
        }
    }
}

struct Inner {
    state: RunState,
    current_step: Option<Step>,
    cancel: Option<CancellationToken>,
    /// Signals the daemon's idle-exit timer that a state transition happened.
    idle_tx: watch::Sender<bool>,
}

pub struct Installer {
    inner: Arc<Mutex<Inner>>,
    idle_rx: watch::Receiver<bool>,
}

impl Installer {
    pub fn new() -> Self {
        let (idle_tx, idle_rx) = watch::channel(true);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: RunState::Idle,
                current_step: None,
                cancel: None,
                idle_tx,
            })),
            idle_rx,
        }
    }

    /// Channel the daemon main loop watches to know when we're idle.
    /// Initial value: true (idle).
    pub fn idle_signal(&self) -> watch::Receiver<bool> {
        self.idle_rx.clone()
    }
}

#[interface(name = "dev.cosmonaut.Installer1")]
impl Installer {
    /// Run the full install pipeline. Returns when the install finishes,
    /// errors out, or is cancelled. Progress is delivered out-of-band via
    /// the `StepChanged` and `LogLine` signals.
    ///
    /// `spec_json` is a serde-serialized [`cosmonaut_engine::InstallSpec`]
    /// — one argument instead of an ever-growing flat tuple. The engine
    /// crate is the single schema owner; GUI and CLI serialize the same
    /// type. The engine re-validates everything (including the partition
    /// plan against a fresh disk probe), so a malformed or stale spec
    /// fails safely.
    async fn install_json(
        &self,
        spec_json: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let spec: InstallSpec = serde_json::from_str(&spec_json)
            .map_err(|e| fdo::Error::InvalidArgs(format!("bad spec JSON: {e}")))?;

        // Reject overlapping installs.
        {
            let mut inner = self.inner.lock().await;
            if inner.state == RunState::Running {
                return Err(fdo::Error::Failed("install already in progress".into()));
            }
            inner.state = RunState::Running;
            inner.current_step = None;
            let token = CancellationToken::new();
            inner.cancel = Some(token.clone());
            let _ = inner.idle_tx.send(false);
        }

        let cancel = self
            .inner
            .lock()
            .await
            .cancel
            .as_ref()
            .expect("cancel set above")
            .clone();

        let (tx, mut rx) = mpsc::channel::<Event>(256);
        let inner = self.inner.clone();

        // Spawn the engine. We re-send Property invalidation + signals as
        // we receive Event from the engine.
        let engine_handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { install(spec, tx, cancel).await }
        });

        // Drain events until the engine closes the channel.
        while let Some(event) = rx.recv().await {
            match event {
                Event::StepChanged { step, detail } => {
                    {
                        let mut inner = inner.lock().await;
                        inner.current_step = Some(step);
                    }
                    let _ = Self::step_changed(&emitter, step.as_str(), &detail).await;
                }
                Event::Log { stream, line } => {
                    // Mirror every subprocess line into the daemon's own log
                    // (journald via the unit's Standard{Output,Error}=journal),
                    // so failure context survives after the GUI goes away.
                    tracing::info!(target: "engine", stream = stream.as_str(), "{line}");
                    let _ = Self::log_line(&emitter, stream.as_str(), &line).await;
                }
                Event::Progress { percent, step } => {
                    let _ = Self::progress(&emitter, percent, step.as_str()).await;
                }
            }
        }

        // Engine finished (or was cancelled).
        let result = engine_handle
            .await
            .map_err(|e| fdo::Error::Failed(format!("engine join: {e}")))?;

        let (success, err_text) = match &result {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e.to_string()),
        };

        {
            let mut inner = inner.lock().await;
            inner.state = if success {
                RunState::Done
            } else {
                RunState::Error
            };
            inner.cancel = None;
            let _ = inner.idle_tx.send(true);
        }

        let _ = Self::completed(&emitter, success, &err_text).await;

        match result {
            Ok(()) => Ok(()),
            Err(cosmonaut_engine::EngineError::Cancelled) => {
                Err(fdo::Error::Failed("install cancelled by Cancel()".into()))
            }
            Err(e) => Err(fdo::Error::Failed(e.to_string())),
        }
    }

    /// Request cancellation. Returns true if there was a running install
    /// and the request was honored, false otherwise. The Install() call
    /// will return shortly with a Cancelled error.
    async fn cancel(&self) -> bool {
        let inner = self.inner.lock().await;
        match inner.cancel.as_ref() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    #[zbus(property)]
    async fn state(&self) -> String {
        self.inner.lock().await.state.as_str().to_owned()
    }

    #[zbus(property)]
    async fn current_step(&self) -> String {
        self.inner
            .lock()
            .await
            .current_step
            .map(Step::as_str)
            .unwrap_or("")
            .to_owned()
    }

    /// Probe all whole disks: partitions, free-space gaps, and detected
    /// operating systems (root-only: OS detection ro-mounts candidate
    /// filesystems). Returns JSON `Vec<cosmonaut_engine::probe::DiskInfo>`.
    /// Refused while an install is running — the probe mounts things.
    async fn probe_disks(&self) -> fdo::Result<String> {
        {
            let inner = self.inner.lock().await;
            if inner.state == RunState::Running {
                return Err(fdo::Error::Failed(
                    "install in progress; probe refused".into(),
                ));
            }
        }
        let disks = crate::probe::probe_disks_with_os()
            .await
            .map_err(|e| fdo::Error::Failed(format!("probe: {e}")))?;
        serde_json::to_string(&disks).map_err(|e| fdo::Error::Failed(format!("serialize: {e}")))
    }

    /// Detect a default IPv4 route so the wizard can auto-skip the
    /// wifi page on already-online systems (typical wired live env).
    async fn is_online(&self) -> fdo::Result<bool> {
        wifi::is_online()
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    /// Detect a TPM2 device on the host. The GUI uses this to gray
    /// out the TPM2-LUKS radio options when no TPM is present (e.g.
    /// QEMU without -tpmdev).
    async fn is_tpm2_available(&self) -> fdo::Result<bool> {
        Ok(wifi::is_tpm2_available())
    }

    /// Scan for visible wireless networks via iwd. Returns
    /// `[(ssid, security, signal, connected), ...]` ordered as iwd
    /// returned them. Errors with `Failed` if no wireless device is
    /// present or iwctl is not installed.
    async fn scan_wifi(&self) -> fdo::Result<Vec<(String, String, u32, bool)>> {
        let device = wifi::first_wireless_device()
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?
            .ok_or_else(|| fdo::Error::Failed("no wireless device available".into()))?;
        let nets = wifi::scan(&device)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        Ok(nets
            .into_iter()
            .map(|n| (n.ssid, n.security, u32::from(n.signal), n.connected))
            .collect())
    }

    /// Connect to `ssid` with `passphrase`. Blocks until iwd reports
    /// success or failure. Polkit-gated (not enforced in the live env's
    /// allow-rule for cosmic-live).
    async fn connect_wifi(&self, ssid: String, passphrase: String) -> fdo::Result<()> {
        let device = wifi::first_wireless_device()
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?
            .ok_or_else(|| fdo::Error::Failed("no wireless device available".into()))?;
        wifi::connect(&device, &ssid, &passphrase)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(signal)]
    async fn step_changed(
        emitter: &SignalEmitter<'_>,
        step: &str,
        detail: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn log_line(emitter: &SignalEmitter<'_>, stream: &str, line: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn progress(emitter: &SignalEmitter<'_>, percent: u8, step: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn completed(emitter: &SignalEmitter<'_>, success: bool, error: &str)
        -> zbus::Result<()>;
}
