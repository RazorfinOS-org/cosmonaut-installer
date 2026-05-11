//! Wizard page modules. Each page exposes a `view()` function that takes
//! the relevant slice of [`crate::app::App`] state and returns an
//! [`cosmic::Element`]. Page-specific actions are folded into the
//! top-level [`crate::app::Message`] enum so the App's `update()` is the
//! single place state changes happen.

use cosmic::Element;
use cosmic::iced::{Alignment, Length, alignment};
use cosmic::widget;

use crate::app::Message;

pub mod confirm;
pub mod disk;
pub mod done;
pub mod encryption;
pub mod image;
pub mod progress;
pub mod welcome;
pub mod wifi;

/// Standard wizard-page chrome: padded container, title at top, optional
/// centered description line, body, and a nav row at the bottom. Mirrors
/// the wifi page's layout so the option pages (Image/Disk/Encryption/
/// Confirm) feel like a single wizard. Welcome/Done/Progress have their
/// own non-wizard shapes and don't go through this helper.
pub fn wizard_frame<'a>(
    title: &'a str,
    description: Option<&'a str>,
    body: Element<'a, Message>,
    nav: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut column = widget::column::with_capacity(4)
        .spacing(20)
        .push(widget::text::title2(title));

    if let Some(desc) = description {
        column = column.push(
            widget::container(
                widget::text::body(desc).align_x(alignment::Horizontal::Center),
            )
            .center_x(Length::Fill),
        );
    }

    let body_container = widget::container(body)
        .width(Length::Fill)
        .height(Length::Fill);

    column = column
        .push(body_container)
        .push(widget::container(nav).align_x(Alignment::End).width(Length::Fill));

    widget::container(column)
        .padding(36)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Image,
    Wifi,
    Disk,
    Encryption,
    Confirm,
    Progress,
    Done,
}

impl Page {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Image => "Image",
            Self::Wifi => "Wifi",
            Self::Disk => "Disk",
            Self::Encryption => "Encryption",
            Self::Confirm => "Confirm",
            Self::Progress => "Installing",
            Self::Done => "Done",
        }
    }
}
