pub mod background;
pub mod git_tasks;
pub mod persistence_thread;
pub mod reaper;
pub mod session_store;
pub mod sidebar;
pub mod state;
pub mod workbench;

use iced::widget::row;
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
        row![sidebar::view(self), workbench::view(self)]
            .height(Length::Fill)
            .into()
    }
}

pub fn run() -> iced::Result {
    // `workbench::subscription` exists and is ready to plug in, but wiring it
    // into `.subscription(...)` here is Task 8's job: boot-time integration
    // is where it gets batched with Task 7's presence-polling subscription
    // (which doesn't exist yet) into one `AppState::subscription` function.
    // Adding a lone `.subscription(workbench::subscription)` now would just
    // mean Task 8 has to replace this line anyway once the second source
    // shows up — so the seam is left here instead of half-wired.
    iced::application(AppState::new, AppState::update, AppState::view)
        .title(AppState::title)
        .window_size(Size {
            width: 1280.0,
            height: 800.0,
        })
        .run()
}
