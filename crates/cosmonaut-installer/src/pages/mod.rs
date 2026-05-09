//! Wizard page modules. Each page exposes a `view()` function that takes
//! the relevant slice of [`crate::app::App`] state and returns an
//! [`cosmic::Element`]. Page-specific actions are folded into the
//! top-level [`crate::app::Message`] enum so the App's `update()` is the
//! single place state changes happen.

pub mod confirm;
pub mod disk;
pub mod done;
pub mod encryption;
pub mod image;
pub mod progress;
pub mod welcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Image,
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
            Self::Disk => "Disk",
            Self::Encryption => "Encryption",
            Self::Confirm => "Confirm",
            Self::Progress => "Installing",
            Self::Done => "Done",
        }
    }
}
