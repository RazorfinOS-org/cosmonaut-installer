use cosmic::app::{Core, Task};
use cosmic::{Application, ApplicationExt, Element};
use futures_util::StreamExt;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;

use cosmonaut_engine::Encryption;

use crate::daemon::{self, DaemonEvent};
use crate::disks::Disk;
use crate::images_json::{self, Catalog, ImageOption};
use crate::pages::{self, Page};
use crate::spec::{EncryptionChoice, FinalSpec};

pub const APP_ID: &str = "dev.cosmonaut.Installer";
const DEFAULT_HOSTNAME: &str = "cosmic";
const REBOOT_COUNTDOWN_SECS: u8 = 30;
const LOG_BUFFER_BYTES: usize = 32 * 1024;

pub struct App {
    core: Core,
    page: Page,

    images: Vec<ImageOption>,
    image_idx: Option<usize>,
    catalog_loaded: bool,

    disks: Vec<Disk>,
    disk_idx: Option<usize>,

    encryption_choice: EncryptionChoice,
    passphrase: String,

    hostname: String,

    // Progress page state
    install_step: Option<String>,
    install_step_detail: String,
    install_log: String,
    install_in_flight: bool,
    cancel_tx: Option<oneshot::Sender<()>>,

    // Done page state
    install_success: bool,
    install_error: String,
    reboot_countdown: Option<u8>,
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
    EncryptionSelected(EncryptionChoice),
    PassphraseChanged(String),

    // Confirm action
    StartInstall,

    // Progress / daemon stream
    DaemonEvent(DaemonEvent),
    CancelInstall,

    // Done page
    RebootTick,
    RebootNow,
    Quit,
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
        let mut app = Self {
            core,
            page: Page::Welcome,
            images: Vec::new(),
            image_idx: None,
            catalog_loaded: false,
            disks: Vec::new(),
            disk_idx: None,
            encryption_choice: EncryptionChoice::None,
            passphrase: String::new(),
            hostname: DEFAULT_HOSTNAME.to_owned(),
            install_step: None,
            install_step_detail: String::new(),
            install_log: String::new(),
            install_in_flight: false,
            cancel_tx: None,
            install_success: false,
            install_error: String::new(),
            reboot_countdown: None,
        };
        let title_task = app.set_window_title("COSMIC Installer".into());

        // Kick off async loads in parallel: image catalog + disk list.
        let load_catalog = Task::perform(
            async {
                tokio::task::spawn_blocking(images_json::load)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
            },
            |r| cosmic::Action::App(Message::CatalogLoaded(r)),
        );
        let load_disks = Task::perform(
            async {
                tokio::task::spawn_blocking(crate::disks::list_blocking)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
            },
            |r| cosmic::Action::App(Message::DisksLoaded(r)),
        );

        (app, Task::batch([title_task, load_catalog, load_disks]))
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Next => {
                self.page = next_page(self.page, self);
                Task::none()
            }
            Message::Back => {
                self.page = back_page(self.page);
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
                Task::none()
            }
            Message::DisksLoaded(Err(e)) => {
                tracing::error!(error = %e, "lsblk failed");
                Task::none()
            }
            Message::RefreshDisks => Task::perform(
                async {
                    tokio::task::spawn_blocking(crate::disks::list_blocking)
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()))
                },
                |r| cosmic::Action::App(Message::DisksLoaded(r)),
            ),

            Message::ImageSelected(i) => {
                self.image_idx = Some(i);
                Task::none()
            }
            Message::DiskSelected(i) => {
                self.disk_idx = Some(i);
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

            Message::StartInstall => {
                let Some(spec) = self.build_final_spec() else {
                    tracing::error!("StartInstall fired without a complete spec");
                    return Task::none();
                };
                let (disk, image, hostname, enc_type, enc_arg) = spec.to_wire();
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DaemonEvent>();
                let (cancel_tx, cancel_rx) = oneshot::channel();
                self.cancel_tx = Some(cancel_tx);
                self.install_in_flight = true;
                self.install_log.clear();
                self.install_step = None;
                self.install_step_detail.clear();
                self.page = Page::Progress;
                daemon::spawn_install(disk, image, hostname, enc_type, enc_arg, tx, cancel_rx);
                Task::stream(
                    UnboundedReceiverStream::new(rx)
                        .map(|e| cosmic::Action::App(Message::DaemonEvent(e))),
                )
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
                DaemonEvent::Completed { success, error } => {
                    self.install_in_flight = false;
                    self.install_success = success;
                    self.install_error = error;
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
            Page::Welcome => pages::welcome::view(),
            Page::Image => pages::image::view(&self.images, self.image_idx),
            Page::Disk => pages::disk::view(&self.disks, self.disk_idx),
            Page::Encryption => {
                pages::encryption::view(&self.encryption_choice, &self.passphrase)
            }
            Page::Confirm => {
                let image = self.image_idx.and_then(|i| self.images.get(i));
                let disk = self.disk_idx.and_then(|i| self.disks.get(i));
                pages::confirm::view(image, disk, &self.encryption_choice, &self.hostname)
            }
            Page::Progress => pages::progress::view(
                self.install_step.as_deref(),
                &self.install_step_detail,
                &self.install_log,
                self.install_in_flight && self.cancellable_now(),
            ),
            Page::Done => pages::done::view(
                self.install_success,
                &self.install_error,
                self.reboot_countdown,
            ),
        }
    }
}

impl App {
    /// Cancel button is active until we hit the bootc step (which is past
    /// the point of clean rollback). Once `install_step` is "bootc",
    /// disable cancel.
    fn cancellable_now(&self) -> bool {
        match self.install_step.as_deref() {
            Some("bootc") | Some("hostname") | Some("bls") | Some("finalize") => false,
            _ => true,
        }
    }

    fn build_final_spec(&self) -> Option<FinalSpec> {
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
        Some(FinalSpec {
            image,
            disk,
            hostname: self.hostname.clone(),
            encryption,
        })
    }
}

fn next_page(current: Page, app: &App) -> Page {
    match current {
        Page::Welcome => {
            // Auto-skip Image page when the catalog has exactly one leaf.
            if app.images.len() <= 1 {
                Page::Disk
            } else {
                Page::Image
            }
        }
        Page::Image => Page::Disk,
        Page::Disk => Page::Encryption,
        Page::Encryption => Page::Confirm,
        Page::Confirm => Page::Progress, // routed via StartInstall
        Page::Progress | Page::Done => current,
    }
}

fn back_page(current: Page) -> Page {
    match current {
        Page::Welcome | Page::Progress | Page::Done => current,
        Page::Image => Page::Welcome,
        Page::Disk => Page::Image,
        Page::Encryption => Page::Disk,
        Page::Confirm => Page::Encryption,
    }
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

