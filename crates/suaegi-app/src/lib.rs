pub mod background;
pub mod git_tasks;
pub mod persistence_thread;
pub mod presence_poll;
pub mod reaper;
pub mod session_store;
pub mod sidebar;
pub mod state;
pub mod workbench;

use iced::widget::row;
use iced::{Element, Length, Size, Subscription};

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

    /// `workbench::subscription`(세션별 generation 피드)과
    /// `presence_poll::subscription`(티어링된 존재 폴링 타이머)을 하나로
    /// 묶는다. 둘 다 앱 전체에 딱 하나씩만 존재해야 하는 구독이므로
    /// `run()`이 이 함수 하나만 `.subscription(...)`에 건다 — 둘을 따로
    /// 걸면 나중에 셋째가 생겼을 때 배선 지점이 두 곳으로 늘어난다.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            workbench::subscription(self),
            presence_poll::subscription(self),
        ])
    }
}

pub fn run() -> iced::Result {
    iced::application(AppState::boot, AppState::update, AppState::view)
        .title(AppState::title)
        .subscription(AppState::subscription)
        .window_size(Size {
            width: 1280.0,
            height: 800.0,
        })
        .run()
}
