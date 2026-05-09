use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, radio, row, scrollable, text};
use cosmic::Element;

use crate::app::Message;
use crate::images_json::ImageOption;

pub fn view<'a>(images: &'a [ImageOption], selected: Option<usize>) -> Element<'a, Message> {
    if images.is_empty() {
        let body = column::with_capacity(2)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(text::title3("No images available"))
            .push(text::body(
                "No /etc/cosmonaut-installer/images.json catalog found, and no fallback. \
                 Aborting.",
            ));
        return container(body).padding(48).into();
    }

    let mut list = column::with_capacity(images.len()).spacing(12);
    for (idx, opt) in images.iter().enumerate() {
        let label = match &opt.desc {
            Some(d) => format!("{}\n{}", opt.name, d),
            None => opt.name.clone(),
        };
        list = list.push(radio(text::body(label), idx, selected, Message::ImageSelected));
    }

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(
            button::suggested("Continue")
                .on_press_maybe(selected.map(|_| Message::Next)),
        );

    let body = column::with_capacity(3)
        .spacing(24)
        .push(text::title2("Choose an image"))
        .push(scrollable(list).height(Length::Fill))
        .push(nav);

    container(body)
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
