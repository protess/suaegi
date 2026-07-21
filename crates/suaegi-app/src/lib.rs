pub mod background;
pub mod git_tasks;
pub mod persistence_thread;
pub mod reaper;
pub mod session_store;
pub mod sidebar;
pub mod state;

use iced::widget::{center, row, text};
use iced::{Element, Length, Size};

pub use state::{AppState, Message, OpId};

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn title(&self) -> String {
        "Suaegi".to_string()
    }
    // The real `update` logic lives on `AppState` in `state.rs` — it dispatches
    // Task 3's git operations, so it needs `&mut self` and the full `Message`
    // match, not a thin wrapper here.
    pub fn view(&self) -> Element<'_, Message> {
        // Task 6 replaces this placeholder with the pane_grid workbench.
        let workbench_placeholder = center(text("Select or create a worktree to start a session"))
            .width(Length::Fill)
            .height(Length::Fill);

        row![sidebar::view(self), workbench_placeholder]
            .height(Length::Fill)
            .into()
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
