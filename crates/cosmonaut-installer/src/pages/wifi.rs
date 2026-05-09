use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, radio, row, scrollable, text, text_input};
use cosmic::Element;

use crate::app::Message;
use crate::daemon::WifiNetwork;

#[derive(Debug, Clone)]
pub enum State {
    /// The wizard hasn't asked for a scan yet (e.g. just navigated in).
    Idle,
    Scanning,
    /// Scan complete; user picks a network.
    Networks(Vec<WifiNetwork>),
    /// User has clicked Connect; iwctl is doing its thing.
    Connecting { ssid: String },
    Error(String),
}

pub fn view<'a>(
    state: &'a State,
    selected: Option<usize>,
    passphrase: &'a str,
) -> Element<'a, Message> {
    let body: Element<Message> = match state {
        State::Idle => column::with_capacity(2)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(text::title2("Wifi"))
            .push(text::body("Scanning for networks…"))
            .into(),

        State::Scanning => column::with_capacity(2)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(text::title2("Wifi"))
            .push(text::body("Scanning for networks…"))
            .into(),

        State::Networks(nets) if nets.is_empty() => column::with_capacity(3)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(text::title2("Wifi"))
            .push(text::body(
                "No wireless networks found. Plug in an ethernet cable, or skip and \
                 connect post-install.",
            ))
            .push(
                row::with_capacity(3)
                    .spacing(12)
                    .push(button::standard("Back").on_press(Message::Back))
                    .push(button::standard("Rescan").on_press(Message::WifiRescan))
                    .push(button::suggested("Skip").on_press(Message::WifiSkip)),
            )
            .into(),

        State::Networks(nets) => {
            let mut list = column::with_capacity(nets.len()).spacing(8);
            for (idx, n) in nets.iter().enumerate() {
                let bars = "▰".repeat(n.signal as usize)
                    + &"▱".repeat(4_usize.saturating_sub(n.signal as usize));
                let suffix = if n.connected { " (connected)" } else { "" };
                let label = format!("{bars}  {}  [{}]{suffix}", n.ssid, n.security);
                list = list.push(radio(text::body(label), idx, selected, Message::WifiSelected));
            }

            let needs_psk = selected
                .and_then(|i| nets.get(i))
                .map(|n| n.security != "open")
                .unwrap_or(false);
            let pass_widget = if needs_psk {
                column::with_capacity(2)
                    .spacing(8)
                    .push(text::body("Passphrase"))
                    .push(
                        text_input("Wifi passphrase", passphrase)
                            .password()
                            .on_input(Message::WifiPassphraseChanged),
                    )
            } else {
                column::with_capacity(0)
            };

            let valid = selected.is_some() && (!needs_psk || !passphrase.is_empty());
            let nav = row::with_capacity(4)
                .spacing(12)
                .push(button::standard("Back").on_press(Message::Back))
                .push(button::standard("Rescan").on_press(Message::WifiRescan))
                .push(button::standard("Skip").on_press(Message::WifiSkip))
                .push(
                    button::suggested("Connect")
                        .on_press_maybe(valid.then_some(Message::WifiConnect)),
                );

            column::with_capacity(4)
                .spacing(20)
                .push(text::title2("Wifi"))
                .push(scrollable(list).height(Length::Fill))
                .push(pass_widget)
                .push(nav)
                .into()
        }

        State::Connecting { ssid } => column::with_capacity(2)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(text::title2("Wifi"))
            .push(text::body(format!("Connecting to {ssid}…")))
            .into(),

        State::Error(msg) => column::with_capacity(3)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(text::title2("Wifi"))
            .push(text::body(format!("Error: {msg}")))
            .push(
                row::with_capacity(3)
                    .spacing(12)
                    .push(button::standard("Back").on_press(Message::Back))
                    .push(button::standard("Rescan").on_press(Message::WifiRescan))
                    .push(button::suggested("Skip").on_press(Message::WifiSkip)),
            )
            .into(),
    };

    container(body)
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
