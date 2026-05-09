use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, text};
use cosmic::Element;

use crate::app::Message;

pub fn view<'a>() -> Element<'a, Message> {
    let body = column::with_capacity(3)
        .spacing(24)
        .align_x(Alignment::Center)
        .push(text::title1("Welcome to COSMIC"))
        .push(text::body(
            "This installer will set up COSMIC on the disk you choose. \
             Existing data on that disk will be erased.",
        ))
        .push(button::suggested("Continue").on_press(Message::Next));

    container(body)
        .padding(48)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}
