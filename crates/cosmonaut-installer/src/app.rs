use cosmic::app::{Core, Task};
use cosmic::iced::Subscription;
use cosmic::{Application, ApplicationExt, Element};
use futures_util::StreamExt;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;

use cosmonaut_engine::{Encryption, InstallSpec, PartitionPlan};

use crate::branding::Branding;
use crate::daemon::{self, DaemonEvent};
use crate::disks::Disk;
use crate::images_json::{self, Catalog, ImageOption};
use crate::pages::layout::{LayoutMsg, LayoutUiState};
use crate::pages::{self, wifi as wifi_page, Page};
use crate::spec::{EncryptionChoice, PartitionModeChoice};

pub const APP_ID: &str = "dev.cosmonaut.Installer";
const DEFAULT_HOSTNAME: &str = "cosmic";
const REBOOT_COUNTDOWN_SECS: u8 = 30;
const LOG_BUFFER_BYTES: usize = 32 * 1024;

pub struct App {
    core: Core,
    page: Page,

    /// Distro-overridable strings (window title, welcome copy, etc.).
    /// Loaded synchronously at startup from `/etc/.../branding.json`
    /// (admin override), `/usr/share/.../branding.json` (vendor default
    /// shipped by cosmic-build-meta), or a built-in fallback. See
    /// [`crate::branding`] for the search order.
    branding: Branding,

    images: Vec<ImageOption>,
    image_idx: Option<usize>,
    catalog_loaded: bool,

    disks: Vec<Disk>,
    disk_idx: Option<usize>,
    /// Partitioning mode for the selected disk. Non-erase modes are
    /// gated behind `COSMONAUT_EXPERIMENTAL_LAYOUT=1` until the
    /// loopback matrix proves them out on real tables.
    partition_mode: PartitionModeChoice,
    experimental_layout: bool,
    /// Custom-layout page state (only meaningful in Custom mode).
    layout: LayoutUiState,

    encryption_choice: EncryptionChoice,
    passphrase: String,

    hostname: String,

    /// All wifi-page state, lifted into its own struct (port of CIS's
    /// `page::wifi::Page`). Lives independently of `wifi_online` below,
    /// which is the wired-network probe used for auto-skip.
    wifi: wifi_page::WifiUiState,
    /// Result of the daemon's is_online probe at startup.
    /// `None` = not probed yet (default to showing the page),
    /// `Some(true)` = wired/online (auto-skip the page),
    /// `Some(false)` = offline (show the page).
    wifi_online: Option<bool>,

    /// Result of the daemon's is_tpm2_available probe.
    /// `None` = not probed yet (encryption page assumes available
    /// optimistically), `Some(false)` = TPM2 radios shown but with a
    /// "no TPM detected" hint, `Some(true)` = no special UI.
    tpm2_available: Option<bool>,

    // Progress page state
    install_step: Option<String>,
    install_step_detail: String,
    install_log: String,
    install_in_flight: bool,
    cancel_tx: Option<oneshot::Sender<()>>,
    /// Overall progress 0–100; None until the first Progress signal.
    install_percent: Option<u8>,
    /// Progress-page log expander ("Show details").
    show_log: bool,
    /// Current branding slide on the progress carousel.
    slide_idx: usize,

    // Done page state
    install_success: bool,
    install_error: String,
    reboot_countdown: Option<u8>,
    /// Step that was active when a failed install died (error page detail).
    failed_step: Option<String>,
    /// Spec of the last attempted install, kept so Retry can re-run it.
    last_spec: Option<InstallSpec>,
    /// Outcome of the last SaveLogs action: Ok(path) or Err(message).
    logs_saved: Option<Result<String, String>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Next,
    Back,

    // Async loaders
    CatalogLoaded(Result<Option<Catalog>, String>),
    DisksLoaded(Result<Vec<Disk>, String>),
    RefreshDisks,

    // Page selections
    ImageSelected(usize),
    DiskSelected(usize),
    PartitionModeSelected(PartitionModeChoice),
    /// Custom-layout page interactions (see `pages::layout::LayoutMsg`).
    Layout(LayoutMsg),
    EncryptionSelected(EncryptionChoice),
    PassphraseChanged(String),

    // Hardware probes
    Tpm2Probed(Result<bool, String>),

    // Wifi page
    WifiOnlineProbed(Result<bool, String>),
    WifiSkip,
    /// All wifi-page interactions are namespaced under this variant so
    /// `App::update`'s match stays manageable. Defined in
    /// `pages::wifi::WifiMsg`.
    Wifi(wifi_page::WifiMsg),

    // Confirm action
    StartInstall,

    // Progress / daemon stream
    DaemonEvent(DaemonEvent),
    CancelInstall,

    // Progress page extras
    ToggleLogView,
    SlideTick,

    // Done page
    RebootTick,
    RebootNow,
    Quit,

    // Failure page actions
    RetryInstall,
    CopyLogs,
    SaveLogs,
    LogsSaved(Result<String, String>),
}

impl Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let branding = Branding::load();
        let title = branding.installer_title.clone();
        let mut app = Self {
            core,
            page: Page::Welcome,
            branding,
            images: Vec::new(),
            image_idx: None,
            catalog_loaded: false,
            disks: Vec::new(),
            disk_idx: None,
            partition_mode: PartitionModeChoice::EraseDisk,
            experimental_layout: std::env::var("COSMONAUT_EXPERIMENTAL_LAYOUT")
                .is_ok_and(|v| v == "1"),
            layout: LayoutUiState::default(),
            encryption_choice: EncryptionChoice::None,
            passphrase: String::new(),
            hostname: DEFAULT_HOSTNAME.to_owned(),
            install_step: None,
            install_step_detail: String::new(),
            install_log: String::new(),
            install_in_flight: false,
            cancel_tx: None,
            install_percent: None,
            show_log: false,
            slide_idx: 0,
            install_success: false,
            install_error: String::new(),
            reboot_countdown: None,
            failed_step: None,
            last_spec: None,
            logs_saved: None,
            wifi: wifi_page::WifiUiState::default(),
            wifi_online: None,
            tpm2_available: None,
        };
        let title_task = app.set_window_title(title);

        // Kick off async loads in parallel: image catalog + disk list +
        // is_online probe (so we can auto-skip the wifi page if wired
        // is already up).
        let load_catalog = Task::perform(
            async {
                tokio::task::spawn_blocking(images_json::load)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
            },
            |r| cosmic::Action::App(Message::CatalogLoaded(r)),
        );
        let load_disks = Task::perform(crate::disks::probe_with_fallback(), |r| {
            cosmic::Action::App(Message::DisksLoaded(r))
        });
        let probe_online = Task::perform(daemon::is_online(), |r| {
            cosmic::Action::App(Message::WifiOnlineProbed(r))
        });
        let probe_tpm2 = Task::perform(daemon::is_tpm2_available(), |r| {
            cosmic::Action::App(Message::Tpm2Probed(r))
        });

        (
            app,
            Task::batch([
                title_task,
                load_catalog,
                load_disks,
                probe_online,
                probe_tpm2,
            ]),
        )
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Next => {
                let prev = self.page;
                self.page = next_page(self.page, self);
                self.on_page_entered(prev)
            }
            Message::Back => {
                self.page = back_page(self.page, self);
                Task::none()
            }

            Message::CatalogLoaded(Ok(Some(catalog))) => {
                self.images = catalog.leaves();
                if self.images.len() == 1 {
                    self.image_idx = Some(0);
                } else if let Some(default) = catalog.default_image {
                    self.image_idx = self.images.iter().position(|i| i.imgref == default);
                }
                self.catalog_loaded = true;
                Task::none()
            }
            Message::CatalogLoaded(Ok(None)) => {
                tracing::warn!("no images.json catalog found; image picker will show empty");
                self.catalog_loaded = true;
                Task::none()
            }
            Message::CatalogLoaded(Err(e)) => {
                tracing::error!(error = %e, "loading images.json failed");
                self.catalog_loaded = true;
                Task::none()
            }

            Message::DisksLoaded(Ok(disks)) => {
                self.disks = disks;
                if self.disks.len() == 1 {
                    self.disk_idx = Some(0);
                }
                // Geometry may have changed; the layout snapshot is stale.
                self.layout = LayoutUiState::default();
                self.apply_dev_page();
                Task::none()
            }
            Message::DisksLoaded(Err(e)) => {
                tracing::error!(error = %e, "lsblk failed");
                Task::none()
            }
            Message::RefreshDisks => Task::perform(crate::disks::probe_with_fallback(), |r| {
                cosmic::Action::App(Message::DisksLoaded(r))
            }),

            Message::ImageSelected(i) => {
                self.image_idx = Some(i);
                Task::none()
            }
            Message::DiskSelected(i) => {
                if self.disk_idx != Some(i) {
                    // Layout state is per-disk; invalidate on change.
                    self.layout = LayoutUiState::default();
                }
                self.disk_idx = Some(i);
                Task::none()
            }
            Message::PartitionModeSelected(mode) => {
                self.partition_mode = mode;
                Task::none()
            }
            Message::Layout(msg) => {
                self.layout.update(msg);
                Task::none()
            }
            Message::EncryptionSelected(c) => {
                if !c.needs_passphrase() {
                    self.passphrase.clear();
                }
                self.encryption_choice = c;
                Task::none()
            }
            Message::PassphraseChanged(p) => {
                self.passphrase = p;
                Task::none()
            }

            Message::WifiOnlineProbed(Ok(is_online)) => {
                self.wifi_online = Some(is_online);
                tracing::info!(is_online, "wifi online probe complete");
                Task::none()
            }
            Message::WifiOnlineProbed(Err(e)) => {
                tracing::warn!(error = %e, "is_online probe failed; assuming offline");
                self.wifi_online = Some(false);
                Task::none()
            }
            Message::Tpm2Probed(Ok(available)) => {
                self.tpm2_available = Some(available);
                tracing::info!(tpm2_available = available, "tpm2 probe complete");
                Task::none()
            }
            Message::Tpm2Probed(Err(e)) => {
                tracing::warn!(error = %e, "tpm2 probe failed; assuming unavailable");
                self.tpm2_available = Some(false);
                Task::none()
            }
            Message::WifiSkip => {
                self.page = page_after_wifi();
                Task::none()
            }
            Message::Wifi(msg) => self.wifi.update(msg),

            Message::StartInstall => {
                let Some(spec) = self.build_install_spec() else {
                    tracing::error!("StartInstall fired without a complete spec");
                    return Task::none();
                };
                self.start_install(spec)
            }
            Message::RetryInstall => match self.last_spec.clone() {
                Some(spec) => self.start_install(spec),
                None => {
                    tracing::error!("RetryInstall fired without a stored spec");
                    Task::none()
                }
            },
            Message::CopyLogs => cosmic::iced::clipboard::write(self.failure_report()),
            Message::SaveLogs => {
                let report = self.failure_report();
                Task::perform(
                    async move {
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let path = format!("/tmp/cosmonaut-install-{ts}.log");
                        tokio::fs::write(&path, report)
                            .await
                            .map(|()| path)
                            .map_err(|e| e.to_string())
                    },
                    |r| cosmic::Action::App(Message::LogsSaved(r)),
                )
            }
            Message::LogsSaved(result) => {
                if let Err(e) = &result {
                    tracing::error!(error = %e, "saving install logs failed");
                }
                self.logs_saved = Some(result);
                Task::none()
            }

            Message::DaemonEvent(event) => match event {
                DaemonEvent::Step { step, detail } => {
                    self.install_step = Some(step);
                    self.install_step_detail = detail;
                    Task::none()
                }
                DaemonEvent::Log { stream, line } => {
                    push_log(&mut self.install_log, &stream, &line);
                    Task::none()
                }
                DaemonEvent::Progress { percent } => {
                    // Monotonic: ignore any out-of-order signal delivery.
                    if self.install_percent.is_none_or(|p| percent > p) {
                        self.install_percent = Some(percent);
                    }
                    Task::none()
                }
                DaemonEvent::Completed { success, error } => {
                    self.install_in_flight = false;
                    self.install_success = success;
                    self.install_error = error;
                    if success {
                        self.install_percent = Some(100);
                    } else {
                        self.failed_step = self.install_step.clone();
                    }
                    self.page = Page::Done;
                    if success {
                        self.reboot_countdown = Some(REBOOT_COUNTDOWN_SECS);
                        return Task::perform(
                            async { tokio::time::sleep(std::time::Duration::from_secs(1)).await },
                            |_| cosmic::Action::App(Message::RebootTick),
                        );
                    }
                    Task::none()
                }
                DaemonEvent::ConnectionError(msg) => {
                    self.install_in_flight = false;
                    self.install_success = false;
                    self.install_error = format!("DBus: {msg}");
                    self.failed_step = self.install_step.clone();
                    self.page = Page::Done;
                    Task::none()
                }
            },
            Message::CancelInstall => {
                if let Some(tx) = self.cancel_tx.take() {
                    let _ = tx.send(());
                }
                Task::none()
            }

            Message::ToggleLogView => {
                self.show_log = !self.show_log;
                Task::none()
            }
            Message::SlideTick => {
                // Stop rotating once the install is over.
                if self.page != Page::Progress || self.branding.slides.is_empty() {
                    return Task::none();
                }
                self.slide_idx = (self.slide_idx + 1) % self.branding.slides.len();
                Task::perform(
                    async { tokio::time::sleep(std::time::Duration::from_secs(12)).await },
                    |_| cosmic::Action::App(Message::SlideTick),
                )
            }

            Message::RebootTick => {
                let Some(c) = self.reboot_countdown else {
                    return Task::none();
                };
                if c <= 1 {
                    self.reboot_countdown = Some(0);
                    daemon::spawn_reboot();
                    Task::none()
                } else {
                    self.reboot_countdown = Some(c - 1);
                    Task::perform(
                        async { tokio::time::sleep(std::time::Duration::from_secs(1)).await },
                        |_| cosmic::Action::App(Message::RebootTick),
                    )
                }
            }
            Message::RebootNow => {
                self.reboot_countdown = Some(0);
                daemon::spawn_reboot();
                Task::none()
            }
            Message::Quit => std::process::exit(0),
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        match self.page {
            Page::Welcome => pages::welcome::view(&self.branding),
            Page::Image => pages::image::view(&self.branding, &self.images, self.image_idx),
            Page::Wifi => pages::wifi::view(&self.wifi, Message::WifiSkip, Message::Back),
            Page::Disk => pages::disk::view(
                &self.disks,
                self.disk_idx,
                self.partition_mode,
                self.experimental_layout,
            ),
            Page::CustomLayout => pages::layout::view(&self.layout),
            Page::Encryption => pages::encryption::view(
                &self.encryption_choice,
                &self.passphrase,
                self.tpm2_available,
            ),
            Page::Confirm => {
                let image = self.image_idx.and_then(|i| self.images.get(i));
                let disk = self.disk_idx.and_then(|i| self.disks.get(i));
                pages::confirm::view(
                    image,
                    disk,
                    self.partition_mode,
                    &self.layout,
                    &self.encryption_choice,
                    &self.hostname,
                )
            }
            Page::Progress => pages::progress::view(
                self.install_step.as_deref(),
                &self.install_step_detail,
                &self.install_log,
                self.install_in_flight && self.cancellable_now(),
                self.install_percent,
                self.show_log,
                self.branding.slides.get(self.slide_idx),
            ),
            Page::Done => pages::done::view(
                &self.branding,
                self.install_success,
                &self.install_error,
                self.failed_step.as_deref(),
                &self.install_log,
                self.logs_saved.as_ref(),
                self.reboot_countdown,
            ),
        }
    }

    /// Surface the wifi page's auth/forget dialogs (port of CIS's
    /// per-page `dialog()` method). Other pages don't have dialogs yet,
    /// so this fans out only when we're actually on the Wifi page.
    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        match self.page {
            Page::Wifi => pages::wifi::dialog(&self.wifi),
            _ => None,
        }
    }

    /// Keep the NetworkManager stream running for the whole app lifetime
    /// so the wifi page's state is already warm by the time the user
    /// reaches it (and so wired→online flips show up even if the page is
    /// auto-skipped).
    fn subscription(&self) -> Subscription<Self::Message> {
        pages::wifi::subscription()
    }
}

impl App {
    /// Dev/screenshot hook: `COSMONAUT_DEV_PAGE=<page>` jumps straight
    /// to a page once the disk probe lands, synthesizing plausible state
    /// where a real install would normally provide it. Never set in
    /// production images. Pages: welcome, image, disk, layout, encryption,
    /// confirm, progress, done-ok, done-fail.
    fn apply_dev_page(&mut self) {
        let Ok(page) = std::env::var("COSMONAUT_DEV_PAGE") else {
            return;
        };
        if self.image_idx.is_none() && !self.images.is_empty() {
            self.image_idx = Some(0);
        }
        if self.disk_idx.is_none() && !self.disks.is_empty() {
            self.disk_idx = Some(0);
        }
        match page.as_str() {
            "image" => self.page = Page::Image,
            "disk" => self.page = Page::Disk,
            "layout" => {
                self.partition_mode = PartitionModeChoice::Custom;
                if let Some(disk) = self.disk_idx.and_then(|i| self.disks.get(i)) {
                    self.layout.reset_for(disk);
                }
                self.page = Page::CustomLayout;
            }
            "encryption" => self.page = Page::Encryption,
            "confirm" => self.page = Page::Confirm,
            "progress" => {
                self.install_step = Some("bootc".into());
                self.install_step_detail =
                    "installing oci:/usr/lib/bootc/install-source/main".into();
                self.install_percent = Some(47);
                self.install_in_flight = true;
                for i in 1..=14 {
                    push_log(
                        &mut self.install_log,
                        "stdout",
                        &format!("Copying blob sha256:{i:064x}"),
                    );
                }
                self.page = Page::Progress;
            }
            "done-ok" => {
                self.install_success = true;
                self.reboot_countdown = Some(23);
                self.page = Page::Done;
            }
            "done-fail" => {
                self.install_success = false;
                self.failed_step = Some("bootc".into());
                self.install_error =
                    "bootc: skopeo copy: writing blob: no space left on device".into();
                for i in 1..=10 {
                    push_log(
                        &mut self.install_log,
                        if i % 3 == 0 { "stderr" } else { "stdout" },
                        &format!("Copying blob sha256:{i:064x}"),
                    );
                }
                self.page = Page::Done;
            }
            _ => {}
        }
    }

    /// Side-effect for transitioning into a new page (called from
    /// `Message::Next`'s handler, *after* `next_page` resolved). The
    /// wifi page uses this hook to spawn its NM secret agent on first
    /// entry; the NM event stream itself runs from `subscription()` so
    /// the access-point list is already populated by the time we get
    /// here.
    fn on_page_entered(&mut self, _from: Page) -> Task<Message> {
        match self.page {
            Page::Wifi => self.wifi.on_first_enter(),
            Page::CustomLayout => {
                // (Re)build layout state for the selected disk when absent
                // or built for a different disk.
                if let Some(disk) = self.disk_idx.and_then(|i| self.disks.get(i)) {
                    let stale = self
                        .layout
                        .disk
                        .as_ref()
                        .is_none_or(|d| d.path != disk.path);
                    if stale {
                        self.layout.reset_for(disk);
                    }
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// Kick off an install of `spec` and switch to the Progress page.
    /// Shared by `StartInstall` (fresh spec from the wizard) and
    /// `RetryInstall` (stored spec after a failure).
    fn start_install(&mut self, spec: InstallSpec) -> Task<Message> {
        self.last_spec = Some(spec.clone());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DaemonEvent>();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        self.cancel_tx = Some(cancel_tx);
        self.install_in_flight = true;
        self.install_log.clear();
        self.install_step = None;
        self.install_step_detail.clear();
        self.failed_step = None;
        self.logs_saved = None;
        self.install_percent = None;
        self.show_log = false;
        self.slide_idx = 0;
        self.page = Page::Progress;
        daemon::spawn_install(spec, tx, cancel_rx);
        let events = Task::stream(
            UnboundedReceiverStream::new(rx).map(|e| cosmic::Action::App(Message::DaemonEvent(e))),
        );
        if self.branding.slides.is_empty() {
            events
        } else {
            let tick = Task::perform(
                async { tokio::time::sleep(std::time::Duration::from_secs(12)).await },
                |_| cosmic::Action::App(Message::SlideTick),
            );
            Task::batch([events, tick])
        }
    }

    /// Plain-text failure report for the Copy/Save logs actions. Includes
    /// the spec summary (passphrase never included), failing step, error,
    /// and the retained log buffer tail.
    fn failure_report(&self) -> String {
        let mut report = String::new();
        report.push_str("cosmonaut-installer failure report\n");
        if let Some(spec) = &self.last_spec {
            report.push_str(&format!("image: {}\n", spec.image));
            report.push_str(&format!("disk: {}\n", spec.disk.display()));
            report.push_str(&format!("hostname: {}\n", spec.hostname));
        }
        report.push_str(&format!("encryption: {}\n", self.encryption_choice.label()));
        report.push_str(&format!(
            "failed step: {}\n",
            self.failed_step.as_deref().unwrap_or("(before first step)")
        ));
        report.push_str(&format!("error: {}\n", self.install_error));
        report.push_str("\n--- log (tail) ---\n");
        report.push_str(&self.install_log);
        report
    }

    /// Cancel button is active until we hit the bootc step (which is past
    /// the point of clean rollback). Once `install_step` is "bootc",
    /// disable cancel.
    fn cancellable_now(&self) -> bool {
        match self.install_step.as_deref() {
            Some("bootc") | Some("hostname") | Some("bls") | Some("finalize") => false,
            _ => true,
        }
    }

    /// The engine plan the current wizard state describes, or None when
    /// the mode needs input that isn't there (no gap, invalid layout).
    fn build_plan(&self) -> Option<PartitionPlan> {
        match self.partition_mode {
            PartitionModeChoice::EraseDisk => Some(PartitionPlan::EraseDisk),
            PartitionModeChoice::FreeSpace => {
                let disk = self.disks.get(self.disk_idx?)?;
                let gap = disk.largest_gap()?;
                Some(PartitionPlan::FreeSpace {
                    gap_start_bytes: gap.start_bytes,
                    gap_size_bytes: gap.size_bytes,
                })
            }
            PartitionModeChoice::Custom => {
                self.layout.validate().ok()?;
                Some(PartitionPlan::Custom {
                    actions: self.layout.to_actions(),
                })
            }
        }
    }

    fn build_install_spec(&self) -> Option<InstallSpec> {
        let image = self.images.get(self.image_idx?)?.imgref.clone();
        let disk = self.disks.get(self.disk_idx?)?.path.clone();
        let encryption = match &self.encryption_choice {
            EncryptionChoice::None => Encryption::None,
            EncryptionChoice::LuksPassphrase => Encryption::LuksPassphrase {
                passphrase: self.passphrase.clone(),
            },
            EncryptionChoice::Tpm2Luks => Encryption::Tpm2Luks,
            EncryptionChoice::Tpm2LuksPassphrase => Encryption::Tpm2LuksPassphrase {
                passphrase: self.passphrase.clone(),
            },
        };
        Some(InstallSpec {
            image,
            disk,
            hostname: self.hostname.clone(),
            encryption,
            plan: self.build_plan()?,
        })
    }
}

fn next_page(current: Page, app: &App) -> Page {
    match current {
        Page::Welcome => {
            // Auto-skip Image page when the catalog has exactly one leaf.
            let after_image = if app.wifi_online == Some(true) {
                page_after_wifi()
            } else {
                Page::Wifi
            };
            if app.images.len() <= 1 {
                after_image
            } else {
                Page::Image
            }
        }
        Page::Image => {
            if app.wifi_online == Some(true) {
                page_after_wifi()
            } else {
                Page::Wifi
            }
        }
        Page::Wifi => page_after_wifi(),
        Page::Disk => {
            if app.partition_mode == PartitionModeChoice::Custom {
                Page::CustomLayout
            } else {
                Page::Encryption
            }
        }
        Page::CustomLayout => Page::Encryption,
        Page::Encryption => Page::Confirm,
        Page::Confirm => Page::Progress, // routed via StartInstall
        Page::Progress | Page::Done => current,
    }
}

fn back_page(current: Page, app: &App) -> Page {
    match current {
        Page::Welcome | Page::Progress | Page::Done => current,
        Page::Image => Page::Welcome,
        Page::Wifi => Page::Image, // (or Welcome if Image was auto-skipped — harmless)
        Page::Disk => Page::Wifi,
        Page::CustomLayout => Page::Disk,
        Page::Encryption => {
            if app.partition_mode == PartitionModeChoice::Custom {
                Page::CustomLayout
            } else {
                Page::Disk
            }
        }
        Page::Confirm => Page::Encryption,
    }
}

fn page_after_wifi() -> Page {
    Page::Disk
}

fn push_log(log: &mut String, stream: &str, line: &str) {
    log.push('[');
    log.push_str(stream);
    log.push_str("] ");
    log.push_str(line);
    log.push('\n');
    if log.len() > LOG_BUFFER_BYTES {
        let drop = log.len() - LOG_BUFFER_BYTES;
        // Drop in line-aligned chunks so we don't show half a line.
        let mut cut = drop;
        if let Some(nl) = log[drop..].find('\n') {
            cut = drop + nl + 1;
        }
        log.drain(..cut);
    }
}
