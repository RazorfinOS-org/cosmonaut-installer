use cosmic::iced::Length;
use cosmic::widget::{button, column, container, radio, row, text, text_input};
use cosmic::Element;

use crate::app::Message;
use crate::spec::EncryptionChoice;

pub fn view<'a>(
    choice: &EncryptionChoice,
    passphrase: &'a str,
    tpm2_available: Option<bool>,
) -> Element<'a, Message> {
    let options = [
        EncryptionChoice::None,
        EncryptionChoice::LuksPassphrase,
        EncryptionChoice::Tpm2Luks,
        EncryptionChoice::Tpm2LuksPassphrase,
    ];

    let mut list = column::with_capacity(options.len()).spacing(8);
    for opt in options {
        list = list.push(radio(
            text::body(opt.label()),
            opt,
            Some(*choice),
            Message::EncryptionSelected,
        ));
    }

    let tpm_hint: Element<Message> = if tpm2_available == Some(false) {
        text::caption("No TPM2 device detected; the TPM2 options will fail at install time.")
            .into()
    } else {
        column::with_capacity(0).into()
    };

    let pass_row = if choice.needs_passphrase() {
        column::with_capacity(2)
            .spacing(8)
            .push(text::body("Passphrase"))
            .push(
                text_input("LUKS passphrase", passphrase)
                    .password()
                    .on_input(Message::PassphraseChanged),
            )
    } else {
        column::with_capacity(0)
    };

    let valid = !choice.needs_passphrase() || !passphrase.is_empty();

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(
            button::suggested("Continue")
                .on_press_maybe(valid.then_some(Message::Next)),
        );

    let body = column::with_capacity(5)
        .spacing(20)
        .push(text::title2("Encryption"))
        .push(list)
        .push(tpm_hint)
        .push(pass_row)
        .push(nav);

    container(body)
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
