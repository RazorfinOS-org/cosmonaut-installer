use cosmic::iced::Length;
use cosmic::widget::{button, column, container, row, text};
use cosmic::Element;

use crate::app::Message;
use crate::disks::Disk;
use crate::images_json::ImageOption;
use crate::spec::EncryptionChoice;

pub fn view<'a>(
    image: Option<&'a ImageOption>,
    disk: Option<&'a Disk>,
    encryption: &EncryptionChoice,
    hostname: &'a str,
) -> Element<'a, Message> {
    let disk_label = disk.map(Disk::label).unwrap_or_else(|| "?".into());
    let image_label = image.map(|i| i.name.as_str()).unwrap_or("?").to_owned();
    let summary = column::with_capacity(4)
        .spacing(8)
        .push(line("Image", image_label))
        .push(line("Disk", disk_label))
        .push(line("Encryption", encryption.label().to_owned()))
        .push(line("Hostname", hostname.to_owned()));

    let warning = text::body(
        "Clicking Install will erase the chosen disk and write the chosen image. \
         This cannot be undone.",
    );

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::destructive("Install").on_press(Message::StartInstall));

    let body = column::with_capacity(4)
        .spacing(24)
        .push(text::title2("Confirm"))
        .push(summary)
        .push(warning)
        .push(nav);

    container(body)
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn line<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    row::with_capacity(2)
        .spacing(12)
        .push(text::body(format!("{label}:")))
        .push(text::body(value))
        .into()
}
