//! Wizard page modules. Each page exposes a `view()` function that takes
//! the relevant slice of [`crate::app::App`] state and returns an
//! [`cosmic::Element`]. Page-specific actions are folded into the
//! top-level [`crate::app::Message`] enum so the App's `update()` is the
//! single place state changes happen.

use cosmic::iced::{Alignment, Border, Length};
use cosmic::{theme, widget, Element};

use crate::app::Message;

pub mod confirm;
pub mod disk;
pub mod done;
pub mod encryption;
pub mod image;
pub mod layout;
pub mod progress;
pub mod welcome;
pub mod wifi;

/// The wizard's decision sequence, as shown on the step rail. The
/// custom-layout page is a sub-step of Disk.
pub const RAIL_STEPS: [&str; 5] = ["Image", "Network", "Disk", "Encryption", "Confirm"];

/// Rail index for a page, when it sits on the rail.
fn rail_index(page: Page) -> Option<usize> {
    match page {
        Page::Image => Some(0),
        Page::Wifi => Some(1),
        Page::Disk | Page::CustomLayout => Some(2),
        Page::Encryption => Some(3),
        Page::Confirm => Some(4),
        _ => None,
    }
}

/// The mission rail: numbered dots for each decision in the sequence,
/// filled up to the current step. Position information, not decoration.
fn step_rail<'a>(current: usize) -> Element<'a, Message> {
    let mut r = widget::row::with_capacity(RAIL_STEPS.len() * 2)
        .spacing(8)
        .align_y(Alignment::Center);
    for (i, label) in RAIL_STEPS.iter().enumerate() {
        let state = i.cmp(&current);
        // Dot: accent-filled for done/current, hollow for upcoming.
        let dot = widget::container(widget::text::body(""))
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0))
            .class(theme::Container::custom(move |t| {
                let accent = t.cosmic().accent_color();
                let neutral = t.cosmic().palette.neutral_5;
                let (bg, alpha) = match state {
                    std::cmp::Ordering::Less => (accent, 0.45),
                    std::cmp::Ordering::Equal => (accent, 1.0),
                    std::cmp::Ordering::Greater => (neutral, 0.5),
                };
                let mut color = cosmic::iced::Color::from(bg);
                color.a = alpha;
                widget::container::Style {
                    background: Some(color.into()),
                    border: Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }));
        let label_el: Element<'a, Message> = if state == std::cmp::Ordering::Equal {
            widget::text::caption_heading(*label)
                .class(theme::Text::Accent)
                .into()
        } else {
            widget::text::caption(*label).into()
        };
        r = r.push(
            widget::row::with_capacity(2)
                .spacing(6)
                .align_y(Alignment::Center)
                .push(dot)
                .push(label_el),
        );
        if i + 1 < RAIL_STEPS.len() {
            // Hairline connector between steps.
            r = r.push(
                widget::container(widget::text::body(""))
                    .width(Length::Fixed(14.0))
                    .height(Length::Fixed(1.0))
                    .class(theme::Container::custom(|t| {
                        let mut c = cosmic::iced::Color::from(t.cosmic().palette.neutral_5);
                        c.a = 0.5;
                        widget::container::Style {
                            background: Some(c.into()),
                            ..Default::default()
                        }
                    })),
            );
        }
    }
    widget::container(
        widget::row::with_capacity(1)
            .push(r)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .into()
}

/// Standard wizard-page chrome: step rail, title, optional description,
/// body, and a nav row at the bottom. Welcome/Done/Progress have their
/// own non-wizard shapes and don't go through this helper.
pub fn wizard_frame<'a>(
    title: &'a str,
    description: Option<&'a str>,
    body: Element<'a, Message>,
    nav: Element<'a, Message>,
    page: Page,
) -> Element<'a, Message> {
    let mut column = widget::column::with_capacity(6).spacing(12);

    if let Some(idx) = rail_index(page) {
        column = column.push(step_rail(idx));
        column = column.push(widget::Space::new().height(Length::Fixed(4.0)));
    }

    column = column.push(widget::text::title2(title));

    if let Some(desc) = description {
        column = column.push(widget::text::body(desc));
    }

    let body_container = widget::container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([12, 0, 0, 0]);

    column = column.push(body_container).push(
        widget::container(nav)
            .align_x(Alignment::End)
            .width(Length::Fill),
    );

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
    CustomLayout,
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
            Self::CustomLayout => "Custom layout",
            Self::Encryption => "Encryption",
            Self::Confirm => "Confirm",
            Self::Progress => "Installing",
            Self::Done => "Done",
        }
    }
}
