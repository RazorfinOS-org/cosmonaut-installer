use cosmic::app::{Core, Task};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, text};
use cosmic::{Application, ApplicationExt, Element};

pub const APP_ID: &str = "dev.cosmonaut.Installer";

pub struct App {
    core: Core,
}

#[derive(Debug, Clone)]
pub enum Message {
    InstallClicked,
}

impl Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let mut app = Self { core };
        let task = app.set_window_title("COSMIC Installer".into());
        (app, task)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::InstallClicked => {
                tracing::info!("Install clicked — Phase 0 stub, no engine wired yet");
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let body = column::with_capacity(3)
            .spacing(24)
            .align_x(Alignment::Center)
            .push(text::title1("Welcome to COSMIC"))
            .push(text::body(
                "This is a Phase 0 placeholder. The install wizard will live here.",
            ))
            .push(button::standard("Install").on_press(Message::InstallClicked));

        container(body)
            .padding(48)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}
