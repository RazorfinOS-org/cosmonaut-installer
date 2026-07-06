//! Install-progress page: percent readout + bar, the branding slide
//! carousel while the user waits, and a collapsible log for the
//! curious. The percent number is the page's anchor — the one thing a
//! user checks from across the room.

use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, progress_bar, row, scrollable, text};
use cosmic::{theme, Element};

use progress_bar::determinate_linear;

use crate::app::Message;
use crate::branding::Slide;

#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    current_step: Option<&'a str>,
    detail: &'a str,
    log: &'a str,
    cancel_enabled: bool,
    percent: Option<u8>,
    show_log: bool,
    slide: Option<&'a Slide>,
) -> Element<'a, Message> {
    let percent_label = percent
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "…".into());
    let header = column::with_capacity(3)
        .spacing(10)
        .push(
            row::with_capacity(2)
                .align_y(Alignment::End)
                .push(
                    column::with_capacity(2)
                        .spacing(4)
                        .push(text::title2("Installing"))
                        .push(text::caption(format!(
                            "{} — {detail}",
                            current_step.unwrap_or("starting")
                        )))
                        .width(Length::Fill),
                )
                .push(text::title1(percent_label).class(theme::Text::Accent)),
        )
        .push(determinate_linear(f32::from(percent.unwrap_or(0)) / 100.0).width(Length::Fill));

    let mut body = column::with_capacity(4).spacing(16).push(header);

    // Branding slide carousel fills the wait when the log is collapsed.
    if !show_log {
        if let Some(s) = slide {
            body = body.push(
                container(
                    column::with_capacity(2)
                        .spacing(12)
                        .align_x(Alignment::Center)
                        .max_width(560.0)
                        .push(text::title3(s.title.clone()))
                        .push(text::body(s.body.clone())),
                )
                .padding(32)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .height(Length::Fill)
                .class(theme::Container::Card),
            );
        } else {
            body = body.push(container(text::body("")).height(Length::Fill));
        }
    }

    // Collapsible log ("details") view.
    if show_log {
        body = body.push(
            container(
                scrollable(text::monotext(log.to_owned()))
                    .anchor_bottom()
                    .height(Length::Fill)
                    .width(Length::Fill),
            )
            .padding(12)
            .height(Length::Fill)
            .class(theme::Container::Card),
        );
    }

    let nav = row::with_capacity(2)
        .align_y(Alignment::Center)
        .push(
            container(
                button::text(if show_log {
                    "Hide details"
                } else {
                    "Show details"
                })
                .on_press(Message::ToggleLogView),
            )
            .width(Length::Fill),
        )
        .push(
            button::standard("Cancel")
                .on_press_maybe(cancel_enabled.then_some(Message::CancelInstall)),
        );

    container(body.push(nav))
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
