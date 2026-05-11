use cosmic::Element;
use cosmic::widget::{button, row, settings, text};

use crate::app::Message;
use crate::disks::Disk;
use crate::images_json::ImageOption;
use crate::pages::wizard_frame;
use crate::spec::EncryptionChoice;

pub fn view<'a>(
    image: Option<&'a ImageOption>,
    disk: Option<&'a Disk>,
    encryption: &EncryptionChoice,
    hostname: &'a str,
) -> Element<'a, Message> {
    let disk_label = disk.map(Disk::label).unwrap_or_else(|| "?".into());
    let image_label = image.map(|i| i.name.as_str()).unwrap_or("?").to_owned();

    let summary = settings::section()
        .title("Summary")
        .add(settings::item::builder("Image").control(text::body(image_label)))
        .add(settings::item::builder("Disk").control(text::body(disk_label)))
        .add(
            settings::item::builder("Encryption")
                .control(text::body(encryption.label().to_owned())),
        )
        .add(settings::item::builder("Hostname").control(text::body(hostname.to_owned())));

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::destructive("Install").on_press(Message::StartInstall));

    wizard_frame(
        "Confirm",
        Some(
            "Clicking Install will erase the chosen disk and write the chosen image. \
             This cannot be undone.",
        ),
        summary.into(),
        nav.into(),
    )
}
