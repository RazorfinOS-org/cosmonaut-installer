use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, icon, row, scrollable, text};
use cosmic::{theme, Element};

use crate::app::Message;
use crate::branding::Branding;

#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    branding: &'a Branding,
    success: bool,
    error: &'a str,
    failed_step: Option<&'a str>,
    log: &'a str,
    logs_saved: Option<&'a Result<String, String>>,
    countdown: Option<u8>,
) -> Element<'a, Message> {
    if success {
        success_view(branding, countdown)
    } else {
        failure_view(error, failed_step, log, logs_saved)
    }
}

fn success_view<'a>(branding: &'a Branding, countdown: Option<u8>) -> Element<'a, Message> {
    let mut body = column::with_capacity(5)
        .spacing(16)
        .align_x(Alignment::Center)
        .max_width(520.0)
        .push(icon::from_name("emblem-default-symbolic").size(56))
        .push(text::title1("Install complete"))
        .push(text::body(branding.done_success_body.as_str()));

    if let Some(s) = countdown {
        body = body.push(text::caption(format!("Restarting in {s} seconds")));
    }

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::suggested("Restart now").on_press(Message::RebootNow))
        .push(button::standard("Quit").on_press(Message::Quit));

    container(body.push(container(nav).padding([12, 0, 0, 0])))
        .padding(48)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// Error page: failing step, error text, the retained log tail, and
/// recovery actions. The log buffer survives past `Completed`, so what's
/// shown here is the same content Copy/Save logs exports.
fn failure_view<'a>(
    error: &'a str,
    failed_step: Option<&'a str>,
    log: &'a str,
    logs_saved: Option<&'a Result<String, String>>,
) -> Element<'a, Message> {
    let header = row::with_capacity(2)
        .spacing(16)
        .align_y(Alignment::Center)
        .push(icon::from_name("dialog-error-symbolic").size(40))
        .push(
            column::with_capacity(2)
                .spacing(4)
                .push(text::title2("Install failed"))
                .push(text::body(format!(
                    "It failed during the “{}” step. Nothing has been left mounted; \
                     retrying is safe.",
                    failed_step.unwrap_or("starting")
                ))),
        );

    let mut body = column::with_capacity(6)
        .spacing(16)
        .push(header)
        .push(text::monotext(error.to_owned()));

    if !log.is_empty() {
        body = body.push(
            container(
                scrollable(text::monotext(log.to_owned()))
                    .anchor_bottom()
                    .height(Length::Fill)
                    .width(Length::Fill),
            )
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .class(theme::Container::Card),
        );
    }

    match logs_saved {
        Some(Ok(path)) => {
            body = body.push(text::body(format!(
                "Logs saved to {path}. The live session is temporary — copy the file \
                 to a USB stick or another machine before rebooting."
            )));
        }
        Some(Err(e)) => {
            body = body.push(text::body(format!("Saving logs failed: {e}")));
        }
        None => {}
    }

    let nav = row::with_capacity(4)
        .spacing(12)
        .push(button::suggested("Retry install").on_press(Message::RetryInstall))
        .push(button::standard("Copy logs").on_press(Message::CopyLogs))
        .push(button::standard("Save logs").on_press(Message::SaveLogs))
        .push(button::standard("Quit").on_press(Message::Quit));

    container(body.push(container(nav).align_x(Alignment::End).width(Length::Fill)))
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
