use tracing_subscriber::EnvFilter;

mod app;
mod branding;
mod daemon;
mod disks;
mod images_json;
mod pages;
mod spec;
mod widgets;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let settings = cosmic::app::Settings::default()
        .size(cosmic::iced::Size::new(960.0, 720.0))
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(640.0)
                .min_height(480.0),
        );

    cosmic::app::run::<app::App>(settings, ())
}
