use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, radio, row, scrollable, settings, text};

use crate::app::Message;
use crate::disks::Disk;
use crate::pages::wizard_frame;

pub fn view<'a>(disks: &'a [Disk], selected: Option<usize>) -> Element<'a, Message> {
    if disks.is_empty() {
        let nav = row::with_capacity(2)
            .spacing(12)
            .push(button::standard("Back").on_press(Message::Back))
            .push(button::suggested("Refresh").on_press(Message::RefreshDisks));
        return wizard_frame(
            "Choose a disk",
            Some(
                "lsblk reported no whole-disk block devices. Connect a target \
                 and click Refresh.",
            ),
            widget::column::with_capacity(0).into(),
            nav.into(),
        );
    }

    let mut section = settings::section().title("Detected disks");
    for (idx, d) in disks.iter().enumerate() {
        section = section.add(radio(
            text::body(d.label()),
            idx,
            selected,
            Message::DiskSelected,
        ));
    }

    let body = scrollable(section).height(Length::Fill).width(Length::Fill);

    let nav = row::with_capacity(3)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::standard("Refresh").on_press(Message::RefreshDisks))
        .push(
            button::suggested("Continue")
                .on_press_maybe(selected.map(|_| Message::Next)),
        );

    wizard_frame(
        "Choose a disk",
        Some("All data on the chosen disk will be erased."),
        body.into(),
        nav.into(),
    )
}
