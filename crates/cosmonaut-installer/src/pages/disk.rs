use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, radio, row, scrollable, text};
use cosmic::Element;

use crate::app::Message;
use crate::disks::Disk;

pub fn view<'a>(disks: &'a [Disk], selected: Option<usize>) -> Element<'a, Message> {
    if disks.is_empty() {
        let body = column::with_capacity(2)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(text::title3("No disks detected"))
            .push(text::body(
                "lsblk reported no whole-disk block devices. Connect a target and \
                 click Refresh.",
            ))
            .push(button::standard("Refresh").on_press(Message::RefreshDisks));
        return container(body).padding(48).into();
    }

    let mut list = column::with_capacity(disks.len()).spacing(12);
    for (idx, d) in disks.iter().enumerate() {
        list = list.push(radio(text::body(d.label()), idx, selected, Message::DiskSelected));
    }

    let nav = row::with_capacity(3)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::standard("Refresh").on_press(Message::RefreshDisks))
        .push(
            button::suggested("Continue")
                .on_press_maybe(selected.map(|_| Message::Next)),
        );

    let body = column::with_capacity(3)
        .spacing(24)
        .push(text::title2("Choose a disk"))
        .push(text::caption(
            "All data on the chosen disk will be erased.",
        ))
        .push(scrollable(list).height(Length::Fill))
        .push(nav);

    container(body)
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
