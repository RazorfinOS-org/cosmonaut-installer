use cosmic::iced::Length;
use cosmic::widget::{self, button, radio, row, scrollable, settings, text};
use cosmic::Element;

use crate::app::Message;
use crate::branding::Branding;
use crate::images_json::ImageOption;
use crate::pages::{wizard_frame, Page};

pub fn view<'a>(
    branding: &'a Branding,
    images: &'a [ImageOption],
    selected: Option<usize>,
) -> Element<'a, Message> {
    if images.is_empty() {
        return wizard_frame(
            "Choose an image",
            Some(
                "No /etc/cosmonaut-installer/images.json catalog found. \
                 The installer can't continue without an image to install.",
            ),
            widget::column::with_capacity(0).into(),
            row::with_capacity(1)
                .push(button::standard("Back").on_press(Message::Back))
                .into(),
            Page::Image,
        );
    }

    let mut section = settings::section().title("Available images");
    for (idx, opt) in images.iter().enumerate() {
        let label = match &opt.desc {
            Some(d) => format!("{}\n{}", opt.name, d),
            None => opt.name.clone(),
        };
        section = section.add(radio(
            text::body(label),
            idx,
            selected,
            Message::ImageSelected,
        ));
    }

    let body = scrollable(section).height(Length::Fill).width(Length::Fill);

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::suggested("Continue").on_press_maybe(selected.map(|_| Message::Next)));

    wizard_frame(
        "Choose an image",
        Some(branding.image_description.as_str()),
        body.into(),
        nav.into(),
        Page::Image,
    )
}
