//! Welcome hero: eyebrow, title, one-line promise, and the mission
//! overview — the same five steps the wizard's rail tracks, so the
//! shape of the journey is visible before it starts.

use cosmic::iced::{Alignment, Border, Length};
use cosmic::widget::{button, column, container, row, text};
use cosmic::{theme, Element};

use crate::app::Message;
use crate::branding::Branding;
use crate::pages::RAIL_STEPS;

const STEP_BLURBS: [&str; 5] = [
    "Pick the system image",
    "Get online (or skip)",
    "Choose where it lives",
    "Encrypt it, or don't",
    "Review and install",
];

pub fn view<'a>(branding: &'a Branding) -> Element<'a, Message> {
    let eyebrow = text::caption_heading(format!("{} INSTALLER", branding.name.to_uppercase()))
        .class(theme::Text::Accent);

    let mut steps = column::with_capacity(RAIL_STEPS.len()).spacing(10);
    for (i, (name, blurb)) in RAIL_STEPS.iter().zip(STEP_BLURBS).enumerate() {
        // The number chip: quiet accent-tinted square, sequence info.
        let chip =
            container(text::caption_heading(format!("{}", i + 1)).class(theme::Text::Accent))
                .width(Length::Fixed(26.0))
                .height(Length::Fixed(26.0))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .class(theme::Container::custom(|t| {
                    let mut c = cosmic::iced::Color::from(t.cosmic().accent_color());
                    c.a = 0.12;
                    container::Style {
                        background: Some(c.into()),
                        border: Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }));
        steps = steps.push(
            row::with_capacity(3)
                .spacing(12)
                .align_y(Alignment::Center)
                .push(chip)
                .push(text::body(*name).width(Length::Fixed(96.0)))
                .push(text::caption(blurb)),
        );
    }

    let hero = column::with_capacity(6)
        .spacing(20)
        .max_width(520.0)
        .push(eyebrow)
        .push(text::title1(branding.welcome_title.as_str()))
        .push(text::body(branding.welcome_body.as_str()))
        .push(container(steps).padding([12, 0, 12, 0]))
        .push(
            row::with_capacity(2)
                .spacing(12)
                .align_y(Alignment::Center)
                .push(button::suggested("Get started").on_press(Message::Next))
                .push(text::caption("About 10 minutes, one reboot.")),
        );

    container(hero)
        .padding(48)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}
