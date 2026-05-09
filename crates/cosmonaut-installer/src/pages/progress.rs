use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, row, scrollable, text};
use cosmic::Element;

use crate::app::Message;

pub fn view<'a>(
    current_step: Option<&'a str>,
    detail: &'a str,
    log: &'a str,
    cancel_enabled: bool,
) -> Element<'a, Message> {
    let header = column::with_capacity(2)
        .spacing(8)
        .push(text::title2("Installing"))
        .push(text::body(format!(
            "Step: {}",
            current_step.unwrap_or("starting…")
        )))
        .push(text::caption(detail.to_owned()));

    let log_view = scrollable(text::monotext(log.to_owned()))
        .height(Length::Fill)
        .width(Length::Fill);

    let nav = row::with_capacity(1)
        .push(
            button::standard("Cancel")
                .on_press_maybe(cancel_enabled.then_some(Message::CancelInstall)),
        )
        .align_y(Alignment::End);

    let body = column::with_capacity(3)
        .spacing(16)
        .push(header)
        .push(log_view)
        .push(nav);

    container(body)
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
