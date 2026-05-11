use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, row, text};
use cosmic::Element;

use crate::app::Message;
use crate::branding::Branding;

pub fn view<'a>(
    branding: &'a Branding,
    success: bool,
    error: &'a str,
    countdown: Option<u8>,
) -> Element<'a, Message> {
    let title = if success {
        "Install complete"
    } else {
        "Install failed"
    };

    let mut body = column::with_capacity(4)
        .spacing(16)
        .align_x(Alignment::Center)
        .push(text::title1(title));

    if success {
        body = body.push(text::body(branding.done_success_body.as_str()));
        if let Some(s) = countdown {
            body = body.push(text::body(format!("Rebooting in {s}s…")));
        }
    } else {
        body = body
            .push(text::body("Install did not complete:"))
            .push(text::monotext(error.to_owned()));
    }

    let nav = if success {
        row::with_capacity(2)
            .spacing(12)
            .push(button::suggested("Reboot now").on_press(Message::RebootNow))
            .push(button::standard("Quit").on_press(Message::Quit))
    } else {
        row::with_capacity(1).push(button::standard("Quit").on_press(Message::Quit))
    };

    body = body.push(nav);

    container(body)
        .padding(48)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}
