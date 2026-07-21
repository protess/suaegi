pub mod background;
pub mod state;

use iced::widget::{center, text};
use iced::{Element, Size};

pub use state::{AppState, Message, OpId};

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn title(&self) -> String {
        "Suaegi".to_string()
    }
    pub fn update(&mut self, _message: Message) {}
    pub fn view(&self) -> Element<'_, Message> {
        center(text("Suaegi")).into()
    }
}

pub fn run() -> iced::Result {
    iced::application(AppState::new, AppState::update, AppState::view)
        .title(AppState::title)
        .window_size(Size {
            width: 1280.0,
            height: 800.0,
        })
        .run()
}
