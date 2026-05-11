use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, radio, row, scrollable, settings, text, text_input};

use crate::app::Message;
use crate::pages::wizard_frame;
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

    let mut choices = settings::section().title("Encryption mode");
    for opt in options {
        choices = choices.add(radio(
            text::body(opt.label()),
            opt,
            Some(*choice),
            Message::EncryptionSelected,
        ));
    }

    let mut body_column = widget::column::with_capacity(3).spacing(16).push(choices);

    if tpm2_available == Some(false) {
        body_column = body_column.push(
            widget::container(text::caption(
                "No TPM2 device detected; the TPM2 options will fail at install time.",
            ))
            .padding([0, 4]),
        );
    }

    if choice.needs_passphrase() {
        let pass_section = settings::section().title("Passphrase").add(
            settings::item::builder("LUKS passphrase").control(
                text_input("Passphrase", passphrase)
                    .password()
                    .on_input(Message::PassphraseChanged),
            ),
        );
        body_column = body_column.push(pass_section);
    }

    let valid = !choice.needs_passphrase() || !passphrase.is_empty();

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(
            button::suggested("Continue")
                .on_press_maybe(valid.then_some(Message::Next)),
        );

    let body = scrollable(body_column).height(Length::Fill).width(Length::Fill);

    wizard_frame(
        "Encryption",
        Some("Pick how the target disk should be encrypted."),
        body.into(),
        nav.into(),
    )
}
