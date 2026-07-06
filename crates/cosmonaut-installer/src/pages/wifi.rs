// Ported from cosmic-initial-setup's `src/page/wifi.rs`
// (Copyright 2025 System76 <info@system76.com>, GPL-3.0-only).
// Adaptations: dropped the `super::Page` trait impl in favour of an
// inherent `WifiUiState` struct; renamed the inner `Message` enum to
// `WifiMsg` and wrapped it via `crate::app::Message::Wifi`; replaced
// Fluent `fl!(...)` calls with inline strings; removed the AddNetwork
// and per-AP Settings affordances (they shell out to
// `nm-connection-editor`, which the live ISO doesn't ship). Otherwise
// the state machine, dialog, and view structure follow CIS verbatim.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, LazyLock},
};

use cosmic::{
    app::Task,
    iced::core::text::Wrapping,
    iced::runtime::widget::operation::focus_next,
    iced::{Alignment, Length},
    widget::{self, icon},
    Apply, Element,
};
use cosmic_settings_network_manager_subscription::{
    self as network_manager,
    available_wifi::{AccessPoint, NetworkType},
    current_networks::ActiveConnectionInfo,
    nm_secret_agent, NetworkManagerState,
};
use eyre::Context;
use futures::{SinkExt, StreamExt};
use secure_string::SecureString;
use tokio::sync::Mutex;

use crate::app::Message as AppMessage;

pub type SecretSender = Arc<Mutex<Option<tokio::sync::oneshot::Sender<SecureString>>>>;

pub static SECURE_INPUT_WIFI: LazyLock<widget::Id> = LazyLock::new(widget::Id::unique);

/// DBus well-known name we register our NetworkManager secret agent under.
/// Picking a name in our own reverse-DNS space avoids stomping on
/// cosmic-settings' secret agent if both ever run in the same session.
const SECRET_AGENT_BUS_NAME: &str = "dev.cosmonaut.Installer.NetworkManager.SecretAgent";

/// All wifi-page state, lifted out of `App` to keep its field count sane.
#[derive(Debug, Default)]
pub struct WifiUiState {
    nm_state: Option<NmState>,
    secret_tx: Option<tokio::sync::mpsc::Sender<nm_secret_agent::Request>>,
    /// When defined, displays connections for the specific device.
    active_device: Option<Arc<network_manager::devices::DeviceInfo>>,
    dialog: Option<WiFiDialog>,
    view_more_popup: Option<network_manager::SSID>,
    connecting: BTreeSet<network_manager::SSID>,
    ssid_to_uuid: BTreeMap<Box<str>, Box<str>>,
    /// Withhold device update if the view more popup is shown.
    withheld_devices: Option<Vec<network_manager::devices::DeviceInfo>>,
    /// Withhold state update if the view more popup is shown.
    withheld_state: Option<NetworkManagerState>,
    /// True once `init()` has been called; we run it exactly once on
    /// first entry to the wifi page.
    initialized: bool,
}

#[derive(Clone, Debug)]
pub enum WifiMsg {
    /// Cancels a dialog.
    CancelDialog,
    /// Connect to a WiFi network access point.
    Connect(network_manager::SSID),
    /// Connect with a password.
    ConnectWithPassword,
    /// Settings for known connections.
    ConnectionSettings(BTreeMap<Box<str>, Box<str>>),
    /// Disconnect from an access point.
    Disconnect(network_manager::SSID),
    /// An error occurred.
    Error(String),
    /// Identity update from the dialog.
    IdentityUpdate(String),
    /// Focus the secure input.
    FocusSecureInput,
    /// Create a dialog to ask for confirmation on forgetting a connection.
    ForgetRequest(network_manager::SSID),
    /// Forget a known access point.
    Forget(network_manager::SSID),
    /// An update from the network manager daemon.
    NetworkManager(network_manager::Event),
    /// Request an auth dialog.
    PasswordRequest(network_manager::SSID),
    /// Update the password from the dialog.
    PasswordUpdate(SecureString),
    /// An update from the secret agent.
    SecretAgent(nm_secret_agent::Event),
    /// Selects a device to display connections from.
    SelectDevice(Arc<network_manager::devices::DeviceInfo>),
    /// Identity submitted from the dialog.
    SubmitIdentity,
    /// Toggles visibility of the password input.
    TogglePasswordVisibility,
    /// Update NetworkManagerState.
    UpdateState(NetworkManagerState),
    /// Update the devices lists.
    UpdateDevices(Vec<network_manager::devices::DeviceInfo>),
    /// Display more options for an access point.
    ViewMore(Option<network_manager::SSID>),
    /// Toggle WiFi access.
    WiFiEnable(bool),
}

impl From<WifiMsg> for AppMessage {
    fn from(message: WifiMsg) -> Self {
        AppMessage::Wifi(message)
    }
}

#[derive(Clone, Debug)]
enum WiFiDialog {
    Forget(network_manager::SSID),
    Password {
        ssid: network_manager::SSID,
        identity: Option<String>,
        password: SecureString,
        password_hidden: bool,
        tx: SecretSender,
    },
}

#[derive(Debug)]
pub struct NmState {
    conn: zbus::Connection,
    sender: futures::channel::mpsc::UnboundedSender<network_manager::Request>,
    state: NetworkManagerState,
    devices: Vec<network_manager::devices::DeviceInfo>,
}

impl WifiUiState {
    /// One-shot init kicked off the first time the user enters the wifi
    /// page. Spawns the NM secret agent and seeds the SSID→UUID map.
    pub fn on_first_enter(&mut self) -> Task<AppMessage> {
        if self.initialized {
            return Task::none();
        }
        self.initialized = true;
        connection_settings(self)
    }

    pub fn update(&mut self, message: WifiMsg) -> Task<AppMessage> {
        let span = tracing::span!(tracing::Level::INFO, "wifi::update");
        let _span = span.enter();

        match message {
            WifiMsg::NetworkManager(network_manager::Event::RequestResponse {
                req,
                state,
                success,
            }) => {
                if !success {
                    tracing::error!(request = ?req, "network-manager request failed");
                }

                match req {
                    network_manager::Request::Authenticate { ssid, identity, .. } => {
                        if success {
                            self.connecting.remove(ssid.as_str());
                        } else {
                            // Request to retry.
                            self.dialog = Some(WiFiDialog::Password {
                                ssid: ssid.into(),
                                identity,
                                password: SecureString::from(""),
                                password_hidden: true,
                                tx: Arc::default(),
                            });
                        }
                    }

                    network_manager::Request::SelectAccessPoint(
                        ssid,
                        network_type,
                        _tx,
                        _interface,
                    ) => {
                        if success || matches!(network_type, NetworkType::Open) {
                            self.connecting.remove(ssid.as_ref());
                        } else {
                            self.dialog = Some(WiFiDialog::Password {
                                ssid,
                                identity: matches!(network_type, NetworkType::EAP)
                                    .then(String::new),
                                password: SecureString::from(""),
                                password_hidden: true,
                                tx: Arc::new(Mutex::new(None)),
                            });
                            return cosmic::task::message(AppMessage::Wifi(
                                WifiMsg::FocusSecureInput,
                            ));
                        }
                    }

                    _ => (),
                }

                self.update_state(state);

                if let Some(NmState { ref conn, .. }) = self.nm_state {
                    return update_devices(conn.clone());
                }
            }

            WifiMsg::UpdateDevices(devices) => {
                self.update_devices(devices);
            }

            WifiMsg::UpdateState(state) => {
                self.update_state(state);
                return connection_settings(self);
            }

            WifiMsg::NetworkManager(
                network_manager::Event::ActiveConns
                | network_manager::Event::Devices
                | network_manager::Event::WiFiEnabled(_)
                | network_manager::Event::WirelessAccessPoints,
            ) => {
                if let Some(NmState { ref conn, .. }) = self.nm_state {
                    return cosmic::Task::batch(vec![
                        update_state(conn.clone()),
                        update_devices(conn.clone()),
                    ]);
                }
            }

            WifiMsg::NetworkManager(network_manager::Event::WiFiCredentials { .. }) => (),

            WifiMsg::ConnectionSettings(settings) => {
                self.ssid_to_uuid = settings;
            }

            WifiMsg::NetworkManager(network_manager::Event::Init {
                conn,
                sender,
                state,
            }) => {
                self.nm_state = Some(NmState {
                    conn: conn.clone(),
                    sender,
                    state,
                    devices: Vec::new(),
                });

                return update_devices(conn);
            }

            WifiMsg::Connect(ssid) => {
                if let Some(nm) = self.nm_state.as_mut() {
                    let Some(ap) = nm
                        .state
                        .wireless_access_points
                        .iter()
                        .chain(nm.state.known_access_points.iter())
                        .find(|ap| ap.ssid == ssid)
                    else {
                        return Task::none();
                    };
                    self.connecting.insert(ssid.clone());
                    _ = nm
                        .sender
                        .unbounded_send(network_manager::Request::SelectAccessPoint(
                            ssid,
                            ap.network_type,
                            self.secret_tx.clone(),
                            self.active_device.as_ref().map(|d| d.interface.clone()),
                        ));
                }
            }

            WifiMsg::IdentityUpdate(new_identity) => {
                if let Some(WiFiDialog::Password {
                    ref mut identity, ..
                }) = self.dialog
                {
                    *identity = Some(new_identity);
                }
            }

            WifiMsg::PasswordRequest(ssid) => {
                if let Some(nm) = self.nm_state.as_mut() {
                    let Some(ap) = nm
                        .state
                        .wireless_access_points
                        .iter()
                        .chain(nm.state.known_access_points.iter())
                        .find(|ap| ap.ssid == ssid)
                    else {
                        return Task::none();
                    };
                    self.dialog = Some(WiFiDialog::Password {
                        ssid,
                        identity: matches!(ap.network_type, NetworkType::EAP).then(String::new),
                        password: SecureString::from(""),
                        password_hidden: true,
                        tx: Arc::default(),
                    });
                    return cosmic::task::message(AppMessage::Wifi(WifiMsg::FocusSecureInput));
                }
            }

            WifiMsg::PasswordUpdate(pass) => {
                if let Some(WiFiDialog::Password {
                    ref mut password, ..
                }) = self.dialog
                {
                    *password = pass;
                }
            }

            WifiMsg::ConnectWithPassword => {
                let Some(dialog) = self.dialog.take() else {
                    return Task::none();
                };

                if let WiFiDialog::Password {
                    ssid,
                    identity,
                    password,
                    tx,
                    ..
                } = dialog
                {
                    if let Some(nm) = self.nm_state.as_mut() {
                        self.connecting.insert(ssid.clone());
                        let nm_sender = nm.sender.clone();
                        let secret_tx = self.secret_tx.clone();
                        let interface = self.active_device.as_ref().map(|d| d.interface.clone());
                        // Fire-and-forget: the work is just a couple of
                        // channel sends — no need to integrate with iced's
                        // task runtime (which would require shaping the
                        // future's output into Action<AppMessage>).
                        tokio::spawn(async move {
                            let mut guard = tx.lock().await;
                            if let Some(tx) = guard.take() {
                                _ = tx.send(password);
                            } else {
                                _ = nm_sender.unbounded_send(
                                    network_manager::Request::Authenticate {
                                        ssid: ssid.to_string(),
                                        identity,
                                        password,
                                        secret_tx,
                                        interface,
                                    },
                                );
                            }
                        });
                        return Task::none();
                    }
                }
            }

            WifiMsg::TogglePasswordVisibility => {
                if let Some(WiFiDialog::Password {
                    ref mut password_hidden,
                    ..
                }) = self.dialog
                {
                    *password_hidden = !*password_hidden;
                }
            }

            WifiMsg::ViewMore(ssid) => {
                self.view_more_popup = ssid;
                if self.view_more_popup.is_none() {
                    self.close_popup_and_apply_updates();
                }
            }

            WifiMsg::Disconnect(ssid) => {
                self.close_popup_and_apply_updates();
                if let Some(nm) = self.nm_state.as_mut() {
                    _ = nm
                        .sender
                        .unbounded_send(network_manager::Request::Disconnect(ssid));
                }
            }

            WifiMsg::ForgetRequest(ssid) => {
                self.dialog = Some(WiFiDialog::Forget(ssid));
                self.view_more_popup = None;
            }

            WifiMsg::Forget(ssid) => {
                self.dialog = None;
                self.close_popup_and_apply_updates();
                if let Some(nm) = self.nm_state.as_mut() {
                    _ = nm
                        .sender
                        .unbounded_send(network_manager::Request::Forget(ssid));
                }
            }

            WifiMsg::SecretAgent(event) => match event {
                nm_secret_agent::Event::RequestSecret {
                    uuid,
                    name,
                    description: _,
                    previous,
                    tx,
                } => {
                    let ssid = self
                        .ssid_to_uuid
                        .iter()
                        .find_map(|(ssid, conn_uuid)| {
                            if conn_uuid.as_ref() == name.as_str() {
                                Some(network_manager::SSID::from(ssid.as_ref()))
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                    let Some(ap): Option<&AccessPoint> = self.nm_state.as_ref().and_then(|nm| {
                        nm.state
                            .wireless_access_points
                            .iter()
                            .chain(nm.state.known_access_points.iter())
                            .find(|ap| ap.ssid == ssid)
                    }) else {
                        tracing::error!(
                            %uuid,
                            %name,
                            "received secret request for unknown connection"
                        );
                        return Task::none();
                    };

                    self.dialog = Some(WiFiDialog::Password {
                        ssid,
                        password: previous,
                        password_hidden: true,
                        identity: matches!(ap.network_type, NetworkType::EAP).then(String::new),
                        tx,
                    });
                    return cosmic::task::message(AppMessage::Wifi(WifiMsg::FocusSecureInput));
                }
                nm_secret_agent::Event::CancelGetSecrets { uuid: _, name: _ } => {
                    self.dialog = self
                        .dialog
                        .take()
                        .filter(|d| !matches!(d, &WiFiDialog::Password { .. }));
                }
                nm_secret_agent::Event::Failed(error) => {
                    tracing::error!(%error, "secret agent failure");
                    if let Some(WiFiDialog::Password {
                        ssid,
                        password,
                        identity,
                        ..
                    }) = self.dialog.take()
                    {
                        self.dialog = Some(WiFiDialog::Password {
                            password,
                            password_hidden: true,
                            tx: Arc::new(Mutex::new(None)),
                            ssid,
                            identity,
                        });
                        return cosmic::task::message(AppMessage::Wifi(WifiMsg::FocusSecureInput));
                    }
                }
            },

            WifiMsg::SubmitIdentity => {
                if self.dialog.is_some() {
                    return focus_next();
                }
            }

            WifiMsg::WiFiEnable(enable) => {
                if let Some(nm) = self.nm_state.as_mut() {
                    _ = nm
                        .sender
                        .unbounded_send(network_manager::Request::SetWiFi(enable));
                    _ = nm.sender.unbounded_send(network_manager::Request::Reload);
                }
            }

            WifiMsg::CancelDialog => {
                self.dialog = None;
            }

            WifiMsg::Error(why) => {
                tracing::error!(why);
            }

            WifiMsg::SelectDevice(device) => {
                // TODO: Per-device wifi connection handling.
                self.active_device = Some(device);
            }

            WifiMsg::FocusSecureInput => {
                // Upstream CIS implements a find_focused → retry loop here
                // (in case the widget isn't in the tree yet when the dialog
                // first renders). The find_focused operation API moved
                // between libcosmic revs; for our installer the single-shot
                // focus is good enough in practice — the dialog is always
                // composed in the same frame the message fires.
                if matches!(self.dialog, Some(WiFiDialog::Password { .. })) {
                    return cosmic::widget::text_input::focus(SECURE_INPUT_WIFI.clone());
                }
            }
        }

        Task::none()
    }

    /// Closes the view more popup and applies any withheld updates.
    fn close_popup_and_apply_updates(&mut self) {
        self.view_more_popup = None;
        if let Some(ref mut nm_state) = self.nm_state {
            if let Some(state) = self.withheld_state.take() {
                nm_state.state = state;
            }

            if let Some(devices) = self.withheld_devices.take() {
                nm_state.devices = devices;
            }
        }
    }

    /// Withholds updates if the view more popup is displayed.
    fn update_devices(&mut self, devices: Vec<network_manager::devices::DeviceInfo>) {
        if let Some(ref mut nm_state) = self.nm_state {
            if self.view_more_popup.is_some() {
                self.withheld_devices = Some(devices);
            } else {
                nm_state.devices = devices;
            }
        }
    }

    /// Withholds updates if the view more popup is displayed.
    fn update_state(&mut self, state: NetworkManagerState) {
        if let Some(ref mut nm_state) = self.nm_state {
            if self.view_more_popup.is_some() {
                self.withheld_state = Some(state);
            } else {
                nm_state.state = state;
            }
        }
    }
}

const WIFI_DESCRIPTION: &str = "Pick a wireless network to connect to before installing. \
     You can skip and connect later if a wired network is already up.";

pub fn view(ui: &WifiUiState, on_skip: AppMessage, on_back: AppMessage) -> Element<'_, AppMessage> {
    let nav: Element<'_, AppMessage> = widget::row::with_capacity(2)
        .spacing(12)
        .push(widget::button::standard("Back").on_press(on_back))
        .push(widget::button::suggested("Skip").on_press(on_skip))
        .into();

    let Some(NmState { ref state, .. }) = ui.nm_state else {
        return crate::pages::wizard_frame(
            "Wifi",
            Some(WIFI_DESCRIPTION),
            widget::container(widget::text::body("Connecting to NetworkManager\u{2026}"))
                .center_x(Length::Fill)
                .into(),
            nav,
            crate::pages::Page::Wifi,
        );
    };

    let theme = cosmic::theme::active();
    let spacing = &theme.cosmic().spacing;

    let wifi_enable = widget::settings::item::builder("Wi\u{2011}Fi")
        .control(widget::toggler(state.wifi_enabled).on_toggle(WifiMsg::WiFiEnable));

    let mut content = widget::column::with_capacity(4)
        .push(widget::list_column().add(wifi_enable))
        .push_maybe(state.airplane_mode.then(|| {
            widget::row::with_capacity(2)
                .push(icon::from_name("airplane-mode-symbolic"))
                .push(widget::text::body("Airplane mode is on"))
                .spacing(8)
                .align_y(Alignment::Center)
                .apply(widget::container)
                .center_x(Length::Fill)
        }));

    if !state.airplane_mode
        && state.known_access_points.is_empty()
        && state.wireless_access_points.is_empty()
    {
        let no_networks_found = widget::container(widget::text::body(
            "No wireless networks visible yet\u{2026}",
        ))
        .center_x(Length::Fill);

        content = content.push(no_networks_found);
    } else {
        let mut has_known = false;
        let mut has_visible = false;

        let (known_networks, visible_networks) = state.wireless_access_points.iter().fold(
            (
                widget::settings::section().title("Known networks"),
                widget::settings::section().title("Visible networks"),
            ),
            |(mut known_networks, mut visible_networks), network| {
                let is_connected = is_connected(state, network);

                let is_known = state
                    .known_access_points
                    .iter()
                    .map(|known| known.ssid.as_ref())
                    .chain(state.active_conns.iter().filter_map(|active| {
                        if let ActiveConnectionInfo::WiFi { name, .. } = active {
                            Some(name.as_str())
                        } else {
                            None
                        }
                    }))
                    .any(|known| known == network.ssid.as_ref());

                // TODO: detect if access point is secured or not.
                let is_encrypted = true;

                let (connect_txt, connect_msg) = if is_connected {
                    ("Connected".to_string(), None)
                } else if ui.connecting.contains(&network.ssid) {
                    ("Connecting\u{2026}".to_string(), None)
                } else {
                    (
                        "Connect".to_string(),
                        Some(if is_known || !is_encrypted {
                            WifiMsg::Connect(network.ssid.clone())
                        } else {
                            WifiMsg::PasswordRequest(network.ssid.clone())
                        }),
                    )
                };

                let identifier = widget::row::with_capacity(3)
                    .push(widget::icon::from_name(wifi_icon(network.strength)))
                    .push_maybe(
                        is_encrypted.then(|| widget::icon::from_name("connection-secure-symbolic")),
                    )
                    .push(widget::text::body(network.ssid.as_ref()).wrapping(Wrapping::Glyph))
                    .spacing(spacing.space_xxs);

                let connect: Element<'_, WifiMsg> = if let Some(msg) = connect_msg {
                    widget::button::text(connect_txt).on_press(msg).into()
                } else {
                    widget::text::body(connect_txt)
                        .align_y(Alignment::Center)
                        .into()
                };

                let view_more_button =
                    widget::button::icon(widget::icon::from_name("view-more-symbolic"));

                let view_more: Option<Element<_>> = if ui
                    .view_more_popup
                    .as_deref()
                    .map_or(false, |id| id == network.ssid.as_ref())
                {
                    widget::popover(view_more_button.on_press(WifiMsg::ViewMore(None)))
                        .position(widget::popover::Position::Bottom)
                        .on_close(WifiMsg::ViewMore(None))
                        .popup({
                            widget::column::with_capacity(2)
                                .push_maybe(is_connected.then(|| {
                                    popup_button(
                                        WifiMsg::Disconnect(network.ssid.clone()),
                                        "Disconnect".to_string(),
                                    )
                                }))
                                .push_maybe(is_known.then(|| {
                                    popup_button(
                                        WifiMsg::ForgetRequest(network.ssid.clone()),
                                        "Forget".to_string(),
                                    )
                                }))
                                .width(Length::Fixed(170.0))
                                .apply(widget::container)
                                .class(cosmic::style::Container::Dialog)
                        })
                        .apply(|e| Some(Element::from(e)))
                } else if is_known || is_connected {
                    view_more_button
                        .on_press(WifiMsg::ViewMore(Some(network.ssid.clone())))
                        .apply(|e| Some(Element::from(e)))
                } else {
                    None
                };

                let controls = widget::row::with_capacity(2)
                    .push(connect)
                    .push_maybe(view_more)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs);

                let widget = widget::settings::item_row(vec![
                    identifier.into(),
                    widget::space().width(Length::Fill).into(),
                    controls.into(),
                ]);

                if is_known {
                    has_known = true;
                    known_networks = known_networks.add(widget);
                } else {
                    has_visible = true;
                    visible_networks = visible_networks.add(widget);
                }

                (known_networks, visible_networks)
            },
        );

        if has_known || has_visible {
            let mut networks = widget::column::with_capacity(2).spacing(spacing.space_l);

            if has_known {
                networks = networks.push(known_networks);
            }

            if has_visible {
                networks = networks.push(visible_networks);
            }

            content = content.push(widget::scrollable(networks));
        }
    };

    let body: Element<'_, AppMessage> = content
        .spacing(spacing.space_l)
        .width(Length::Fill)
        .apply(Element::from)
        .map(AppMessage::Wifi);

    crate::pages::wizard_frame(
        "Wifi",
        Some(WIFI_DESCRIPTION),
        body,
        nav,
        crate::pages::Page::Wifi,
    )
}

pub fn dialog(ui: &WifiUiState) -> Option<Element<'_, AppMessage>> {
    ui.dialog.as_ref().map(|dialog| match dialog {
        WiFiDialog::Password {
            password,
            identity,
            password_hidden,
            ..
        } => {
            let password = widget::text_input::secure_input(
                "Password",
                password.unsecure(),
                Some(WifiMsg::TogglePasswordVisibility.into()),
                *password_hidden,
            )
            .id(SECURE_INPUT_WIFI.clone())
            .on_input(|input| WifiMsg::PasswordUpdate(SecureString::from(input)).into())
            .on_submit(|_| WifiMsg::ConnectWithPassword.into());

            let primary_action =
                widget::button::suggested("Connect").on_press(WifiMsg::ConnectWithPassword.into());

            let secondary_action =
                widget::button::standard("Cancel").on_press(WifiMsg::CancelDialog.into());

            let control: Element<'_, AppMessage> = if let Some(identity) = identity {
                widget::column::with_capacity(2)
                    .spacing(8)
                    .push(
                        widget::text_input::text_input("Identity", identity)
                            .on_input(|identity| WifiMsg::IdentityUpdate(identity).into())
                            .on_submit(|_| WifiMsg::SubmitIdentity.into()),
                    )
                    .push(password)
                    .into()
            } else {
                password.into()
            };

            widget::dialog()
                .title("Authentication required")
                .icon(icon::from_name("preferences-wireless-symbolic").size(64))
                .body("This network needs a password to join.")
                .control(control)
                .primary_action(primary_action)
                .secondary_action(secondary_action)
                .apply(Element::from)
        }

        WiFiDialog::Forget(ssid) => {
            let primary_action = widget::button::destructive("Forget")
                .on_press(WifiMsg::Forget(ssid.clone()).into());

            let secondary_action =
                widget::button::standard("Cancel").on_press(WifiMsg::CancelDialog.into());

            widget::dialog()
                .title("Forget this Wi-Fi network?")
                .icon(icon::from_name("dialog-information").size(64))
                .body("Saved credentials for this network will be removed.")
                .primary_action(primary_action)
                .secondary_action(secondary_action)
                .apply(Element::from)
        }
    })
}

/// Subscription that bridges NetworkManager DBus signals into the App's
/// message loop. Wire this in `App::subscription()`.
pub fn subscription() -> cosmic::iced::Subscription<AppMessage> {
    cosmic::iced::Subscription::run(network_manager_stream)
}

fn network_manager_stream() -> impl futures::Stream<Item = AppMessage> {
    cosmic::iced::stream::channel::<AppMessage>(
        1,
        |mut output: futures::channel::mpsc::Sender<AppMessage>| async move {
            let conn = match zbus::Connection::system().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(?e, "wifi: failed to connect to system bus");
                    return;
                }
            };

            let (tx, mut rx) = futures::channel::mpsc::channel(1);

            let watchers = std::pin::pin!(async move {
                futures::join!(
                    network_manager::watch(conn.clone(), tx.clone()),
                    network_manager::active_conns::watch(conn.clone(), tx.clone()),
                    network_manager::wireless_enabled::watch(conn.clone(), tx.clone()),
                    network_manager::watch_connections_changed(conn, tx)
                );
            });

            let forwarder = std::pin::pin!(async move {
                while let Some(message) = rx.next().await {
                    _ = output
                        .send(AppMessage::Wifi(WifiMsg::NetworkManager(message)))
                        .await;
                }
            });

            futures::future::select(watchers, forwarder).await;
        },
    )
}

fn is_connected(state: &NetworkManagerState, network: &AccessPoint) -> bool {
    state.active_conns.iter().any(|active| {
        if let ActiveConnectionInfo::WiFi { name, .. } = active {
            *name == network.ssid.as_ref()
        } else {
            false
        }
    })
}

fn popup_button(message: WifiMsg, text: String) -> Element<'static, WifiMsg> {
    let theme = cosmic::theme::active();
    let theme = theme.cosmic();
    widget::text::body(text)
        .align_y(Alignment::Center)
        .apply(widget::button::custom)
        .padding([theme.space_xxxs(), theme.space_xs()])
        .width(Length::Fill)
        .class(cosmic::theme::Button::MenuItem)
        .on_press(message)
        .into()
}

fn connection_settings(page: &mut WifiUiState) -> Task<AppMessage> {
    let settings = async move {
        let conn = zbus::Connection::system().await?;
        let settings = network_manager::dbus::settings::NetworkManagerSettings::new(&conn).await?;

        _ = settings.load_connections(&[]).await;

        let settings = settings
            .list_connections()
            .await?
            .into_iter()
            .map(|conn| async move { conn })
            .apply(futures::stream::FuturesOrdered::from_iter)
            .filter_map(|conn| async move {
                conn.get_settings()
                    .await
                    .map(network_manager::Settings::new)
                    .ok()
            })
            .fold(BTreeMap::new(), |mut set, settings| async move {
                if let Some(ref wifi) = settings.wifi {
                    if let Some(ssid) = wifi
                        .ssid
                        .clone()
                        .and_then(|ssid| String::from_utf8(ssid).ok())
                    {
                        if let Some(ref connection) = settings.connection {
                            if let Some(uuid) = connection.uuid.clone() {
                                set.insert(ssid.into(), uuid.into());
                                return set;
                            }
                        }
                    }
                }

                set
            })
            .await;

        Ok::<_, zbus::Error>(settings)
    };

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    page.secret_tx = Some(tx);

    let conn_settings = cosmic::task::future(async move {
        settings
            .await
            .context("failed to get connection settings")
            .map_or_else(
                |why| WifiMsg::Error(why.to_string()),
                WifiMsg::ConnectionSettings,
            )
            .apply(AppMessage::Wifi)
    });

    let secret_agent = cosmic::Task::stream(
        cosmic_settings_network_manager_subscription::nm_secret_agent::secret_agent_stream(
            SECRET_AGENT_BUS_NAME,
            rx,
        ),
    )
    .map(|m| AppMessage::Wifi(WifiMsg::SecretAgent(m)));

    cosmic::task::batch([conn_settings, secret_agent])
}

pub fn update_state(conn: zbus::Connection) -> Task<AppMessage> {
    cosmic::task::future(async move {
        match NetworkManagerState::new(&conn).await {
            Ok(state) => AppMessage::Wifi(WifiMsg::UpdateState(state)),
            Err(why) => AppMessage::Wifi(WifiMsg::Error(why.to_string())),
        }
    })
}

pub fn update_devices(conn: zbus::Connection) -> Task<AppMessage> {
    cosmic::task::future(async move {
        let filter =
            |device_type| matches!(device_type, network_manager::devices::DeviceType::Wifi);
        match network_manager::devices::list(&conn, filter).await {
            Ok(devices) => AppMessage::Wifi(WifiMsg::UpdateDevices(devices)),
            Err(why) => AppMessage::Wifi(WifiMsg::Error(why.to_string())),
        }
    })
}

fn wifi_icon(strength: u8) -> &'static str {
    if strength < 25 {
        "network-wireless-signal-weak-symbolic"
    } else if strength < 50 {
        "network-wireless-signal-ok-symbolic"
    } else if strength < 75 {
        "network-wireless-signal-good-symbolic"
    } else {
        "network-wireless-signal-excellent-symbolic"
    }
}
