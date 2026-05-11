use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, text};
use cosmic::Element;

use crate::app::Message;
use crate::branding::Branding;

pub fn view<'a>(branding: &'a Branding) -> Element<'a, Message> {
    let body = column::with_capacity(3)
        .spacing(24)
        .align_x(Alignment::Center)
        .push(text::title1(branding.welcome_title.as_str()))
        .push(text::body(branding.welcome_body.as_str()))
        .push(button::suggested("Continue").on_press(Message::Next));

    container(body)
        .padding(48)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}
